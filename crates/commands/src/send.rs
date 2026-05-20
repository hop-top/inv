//! `send_invoice` command.
//!
//! Dispatches the rendered invoice to a destination URI. v1 implements
//! `file://` (write bytes to a path on disk) and `stdout` (write bytes
//! to the [`SendSink`] injected at the call site). The other schemes
//! (`bus://`, `webhook://`, `link://`) are recognised but return
//! [`CoreError::NotImplemented`] — they land in T-0013 (channels).
//!
//! ## FSM
//!
//! `Issued → Sent` and `Viewed → Sent`. Re-sending an already-`Sent`
//! invoice is a self-edge (FSM transition table rejects it as illegal;
//! at v1 we explicitly allow it as a no-op on the state but a new
//! history row + `inv.billing.invoice.sent` event is still recorded so
//! the audit trail captures every send).

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde_json::json;

use inv_core::domain::ids::{HistoryId, InvoiceId};
use inv_core::domain::invoice::{
    HistoryChannel, Invoice, InvoiceLine, InvoiceState, InvoiceStateHistory,
};
use inv_core::render::{render_html, render_pdf, RenderContext};
use inv_core::state::transitions::{next_state, InvoiceEvent};
use inv_core::state::{
    InvoiceEntered, InvoiceProposed, InvoiceTransitioned, TOPIC_ENTERED, TOPIC_PROPOSED,
    TOPIC_TRANSITIONED,
};
use inv_store::repo::customer::CustomerRepo;
use inv_store::repo::history::InvoiceHistoryRepo;
use inv_store::repo::invoice::{InvoiceLineRepo, InvoiceRepo};

use crate::ctx::{Actor, Channel, CoreCtx};
use crate::draft::history_channel_str;
use crate::error::CoreError;
use crate::events::EmittedEvent;

/// Pluggable sink for `stdout` / in-memory delivery.
///
/// The default impl writes to actual stdout. Tests inject a
/// `Vec<u8>`-backed sink so they can assert on the rendered bytes
/// without touching the real fd.
pub trait SendSink: Send + Sync {
    /// Write the rendered bytes (HTML at v1; PDF in 1.1).
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()>;
}

impl SendSink for Vec<u8> {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        std::io::Write::write_all(self, bytes)
    }
}

/// Default sink — writes to the process's real stdout.
#[derive(Debug, Default, Clone, Copy)]
pub struct StdoutSink;

impl SendSink for StdoutSink {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        std::io::stdout().write_all(bytes)
    }
}

/// Input for [`send_invoice`].
pub struct SendInvoiceInput<'s> {
    /// Invoice to send.
    pub invoice_id: InvoiceId,
    /// Destination URI. Supported schemes:
    ///
    /// - `file://<path>` (absolute path)
    /// - `stdout`
    ///
    /// Unsupported (return [`CoreError::NotImplemented`]):
    ///
    /// - `bus://...`, `webhook://...`, `link://...`
    pub destination_uri: String,
    /// Caller-supplied dedupe key (advisory at v1 — same comment as
    /// issue).
    pub idempotency_key: Option<String>,
    /// Who triggered.
    pub actor: Actor,
    /// Channel the request arrived on.
    pub channel: Channel,
    /// Sink for the `stdout` scheme. Ignored for other schemes. Owned
    /// by the caller so tests can pin a `Vec<u8>`.
    pub sink: Option<&'s mut dyn SendSink>,
}

impl SendInvoiceInput<'_> {
    /// Pure validation.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.destination_uri.trim().is_empty() {
            return Err(CoreError::Validation(
                "destination_uri must not be empty".into(),
            ));
        }
        Ok(())
    }
}

/// Output of [`send_invoice`].
#[derive(Debug, Clone)]
pub struct SendInvoiceOutput {
    /// The invoice (now in `Sent`).
    pub invoice: Invoice,
    /// The lines (unchanged from issue).
    pub lines: Vec<InvoiceLine>,
    /// HTML rendered for delivery.
    pub html: String,
    /// PDF bytes delivered (stub engine returns HTML).
    pub pdf: Vec<u8>,
    /// Bus events the command would emit.
    pub emitted_events: Vec<EmittedEvent>,
    /// Resolved destination (`file:///tmp/x.html`, `stdout`, …) for
    /// audit / display.
    pub delivered_to: String,
}

/// Send an issued (or already-sent) invoice via the URI scheme.
#[tracing::instrument(skip_all, fields(
    invoice_id = %input.invoice_id,
    destination = %input.destination_uri
))]
pub async fn send_invoice(
    ctx: &CoreCtx,
    mut input: SendInvoiceInput<'_>,
) -> Result<SendInvoiceOutput, CoreError> {
    input.validate()?;

    // Validate the scheme up front so we never half-mutate on an
    // unsupported destination.
    let scheme = scheme_of(&input.destination_uri);
    match scheme.as_str() {
        "file" | "stdout" => {}
        "bus" | "webhook" | "link" => {
            return Err(CoreError::NotImplemented(format!(
                "send_invoice: {scheme}:// dispatch lands in T-0013"
            )));
        }
        other => {
            return Err(CoreError::Validation(format!(
                "send_invoice: unknown destination scheme `{other}`"
            )));
        }
    }

    let inv_repo = InvoiceRepo::new(&ctx.db);
    let line_repo = InvoiceLineRepo::new(&ctx.db);
    let cust_repo = CustomerRepo::new(&ctx.db);
    let hist_repo = InvoiceHistoryRepo::new(&ctx.db);

    let mut invoice = inv_repo
        .get(&input.invoice_id)
        .await?
        .ok_or_else(|| CoreError::NotFound(format!("invoice {}", input.invoice_id)))?;
    // After fetching we don't use the repo's mutating API; the focused
    // UPDATE in `update_invoice_for_send` is what persists changes.
    let _ = &inv_repo;
    let lines = line_repo.list_for_invoice(&input.invoice_id).await?;

    // FSM: Issued|Viewed → Sent. Sent → Sent is a "resend" — we permit
    // it explicitly here (it isn't legal in the FSM table). The history
    // row + event still fire so the audit trail records every send.
    let from_state = invoice.state;
    let to_state = match from_state {
        InvoiceState::Issued | InvoiceState::Viewed => {
            // Use the FSM table as the source of truth (will yield Sent).
            next_state(from_state, &InvoiceEvent::Send)?
        }
        InvoiceState::Sent => InvoiceState::Sent,
        other => {
            return Err(CoreError::FsmTransition(
                inv_core::state::TransitionError::Illegal {
                    state: other,
                    event: "send",
                },
            ));
        }
    };

    let customer = cust_repo
        .get(&invoice.customer_id)
        .await?
        .ok_or_else(|| {
            CoreError::NotFound(format!(
                "customer {} (referenced by invoice {})",
                invoice.customer_id, input.invoice_id
            ))
        })?;

    let now = ctx.clock.now();

    // Render (HTML always; PDF via stub engine). See issue.rs for why
    // we synthesise a placeholder metadata entry for the render
    // context only.
    let mut render_invoice = invoice.clone();
    if render_invoice.metadata.is_empty() {
        render_invoice
            .metadata
            .insert("_render_placeholder".to_string(), String::new());
    }
    let render_ctx = RenderContext::new(render_invoice, customer, lines.clone());
    let template_path = invoice
        .template_path
        .as_deref()
        .map(std::path::Path::new);
    let html = render_html(template_path, &render_ctx).await?;
    let pdf = render_pdf(&html).await?;

    // Dispatch.
    let delivered_to = match scheme.as_str() {
        "file" => {
            let path = file_path_from_uri(&input.destination_uri)?;
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        CoreError::Validation(format!(
                            "creating directory {}: {e}",
                            parent.display()
                        ))
                    })?;
                }
            }
            std::fs::write(&path, &pdf).map_err(|e| {
                CoreError::Validation(format!("writing {}: {e}", path.display()))
            })?;
            format!("file://{}", path.display())
        }
        "stdout" => {
            let sink = input.sink.as_deref_mut().ok_or_else(|| {
                CoreError::Validation(
                    "send_invoice: stdout destination requires a sink (none supplied)".into(),
                )
            })?;
            sink.write_all(&pdf).map_err(|e| {
                CoreError::Validation(format!("writing to stdout sink: {e}"))
            })?;
            "stdout".to_string()
        }
        _ => unreachable!("scheme guard above"),
    };

    // State mutation + audit row in a single sqlx transaction
    // (design §3.5). InvoiceRepo::save_in_tx is an upsert and does
    // not CASCADE-wipe history rows (T-0024).
    invoice.state = to_state;
    invoice.sent_at = Some(now);
    invoice.updated_at = now;

    let history = InvoiceStateHistory {
        id: HistoryId::new(),
        invoice_id: invoice.id.clone(),
        from_state: Some(from_state),
        to_state,
        event: InvoiceEvent::Send.tag().to_string(),
        actor: Some(input.actor.audit_string()),
        channel: HistoryChannel::from(input.channel),
        bus_event_id: None,
        reason: None,
        occurred_at: now,
        published_at: None,
        metadata: {
            let mut m = BTreeMap::new();
            m.insert("destination".to_string(), delivered_to.clone());
            m
        },
    };
    {
        let mut tx = ctx.db.begin().await.map_err(inv_store::StoreError::from)?;
        InvoiceRepo::save_in_tx(&mut tx, &invoice).await?;
        InvoiceHistoryRepo::save_in_tx(&mut tx, &history).await?;
        tx.commit().await.map_err(inv_store::StoreError::from)?;
    }

    // Bus events.
    let emitted =
        build_sent_events(&invoice, from_state, to_state, &input.actor, history.channel, now, &delivered_to);

    Ok(SendInvoiceOutput {
        invoice,
        lines,
        html,
        pdf,
        emitted_events: emitted,
        delivered_to,
    })
}

fn scheme_of(uri: &str) -> String {
    let head = uri.split("://").next().unwrap_or(uri);
    if head == uri && uri == "stdout" {
        return "stdout".to_string();
    }
    head.to_ascii_lowercase()
}

fn file_path_from_uri(uri: &str) -> Result<PathBuf, CoreError> {
    let rest = uri
        .strip_prefix("file://")
        .ok_or_else(|| CoreError::Validation(format!("file URI: missing `file://` prefix: {uri}")))?;
    if rest.is_empty() {
        return Err(CoreError::Validation(
            "file URI: empty path component".into(),
        ));
    }
    Ok(PathBuf::from(rest))
}

#[allow(clippy::too_many_arguments)]
fn build_sent_events(
    invoice: &Invoice,
    from: InvoiceState,
    to: InvoiceState,
    actor: &Actor,
    channel: HistoryChannel,
    now: DateTime<Utc>,
    destination: &str,
) -> Vec<EmittedEvent> {
    let actor_audit = actor.audit_string();
    let event = InvoiceEvent::Send;

    let proposed = InvoiceProposed::new(
        invoice.id.clone(),
        from,
        to,
        event.clone(),
        Some(actor_audit.clone()),
        channel,
        now,
    );
    let transitioned = InvoiceTransitioned::new(
        invoice.id.clone(),
        from,
        to,
        event.clone(),
        Some(actor_audit.clone()),
        channel,
        now,
    );
    let entered =
        InvoiceEntered::new(invoice.id.clone(), to, channel, Some(actor_audit.clone()), now);

    vec![
        EmittedEvent::new(
            TOPIC_PROPOSED,
            serde_json::to_value(&proposed).unwrap_or(json!({})),
            now,
        ),
        EmittedEvent::new(
            TOPIC_TRANSITIONED,
            serde_json::to_value(&transitioned).unwrap_or(json!({})),
            now,
        ),
        EmittedEvent::new(
            TOPIC_ENTERED,
            serde_json::to_value(&entered).unwrap_or(json!({})),
            now,
        ),
        EmittedEvent::new(
            "inv.billing.invoice.sent",
            json!({
                "invoice_id": invoice.id.to_string(),
                "number": invoice.number,
                "destination": destination,
                "actor": actor_audit,
                "channel": history_channel_str(channel),
            }),
            now,
        ),
    ]
}
