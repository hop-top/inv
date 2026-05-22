//! `send_invoice` command.
//!
//! Dispatches the rendered invoice to a destination URI. v1 implements
//! `file://` (write bytes to a path on disk) and `stdout` (write bytes
//! to the [`SendSink`] injected at the call site). The other schemes
//! (`bus://`, `webhook://`, `link://`) are recognised but return
//! [`CoreError::NotImplemented`] — they land in T-0013 (channels).
//!
//! Channel adapters that own a transport the command layer doesn't
//! understand (HTTP for `webhook://`, signed-link minting for `link://`,
//! a bus publisher for `bus://`) call [`send_invoice_render`] instead.
//! That sibling returns the rendered bytes + records the real
//! destination string on the history row, leaving the actual dispatch
//! to the adapter.
//!
//! ## FSM
//!
//! `Issued → Sent` and `Viewed → Sent`. Re-sending an already-`Sent`
//! invoice is a self-edge (FSM transition table rejects it as illegal;
//! at v1 we explicitly allow it as a no-op on the state but a new
//! history row + `inv.billing.invoice.sent` event is still recorded so
//! the audit trail captures every send).
//!
//! ## Idempotency (T-0037)
//!
//! When `idempotency_key` is set, we look up
//! `(invoice_id, idempotency_key)` in the `send_idempotency` table
//! before doing anything. On a hit the command returns the same
//! [`SendInvoiceOutput`] shape with `idempotency_replay: true`, no FSM
//! mutation, no new history row, no emitted events, and no sink write
//! (`file://` doesn't re-write the bytes; `stdout` doesn't push to the
//! sink). The rendered HTML / PDF are re-computed from the current
//! (post-send) invoice state — deterministic given the frozen tax
//! snapshot — so callers can still display them.
//!
//! The idempotency key is scoped per-invoice: different invoices with
//! the same key never collide.

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
use inv_store::repo::send_idempotency::{SendIdempotencyRecord, SendIdempotencyRepo};

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
    /// Bus events the command would emit. Empty on idempotency replay.
    pub emitted_events: Vec<EmittedEvent>,
    /// Resolved destination (`file:///tmp/x.html`, `stdout`, …) for
    /// audit / display.
    pub delivered_to: String,
    /// True if the command short-circuited on an idempotency-key hit
    /// (no FSM mutation, no new history row, no sink write, no events).
    /// `invoice` / `lines` reflect the current (post-original-send)
    /// state; `html` / `pdf` are re-rendered from that state.
    pub idempotency_replay: bool,
}

/// Input for [`send_invoice_render`].
///
/// Mirror of [`SendInvoiceInput`] with the sink stripped — the
/// render-only path doesn't write anywhere. The destination URI is
/// recorded verbatim on the history row so a `webhook://example.com/x`
/// shows up as `webhook://example.com/x` in the audit trail (not as
/// `stdout`, which is what the v1 work-around persisted).
#[derive(Debug, Clone)]
pub struct SendInvoiceRenderInput {
    /// Invoice to send.
    pub invoice_id: InvoiceId,
    /// Destination URI. Recorded verbatim — no scheme validation.
    pub destination_uri: String,
    /// Caller-supplied dedupe key.
    pub idempotency_key: Option<String>,
    /// Who triggered.
    pub actor: Actor,
    /// Channel the request arrived on.
    pub channel: Channel,
}

impl SendInvoiceRenderInput {
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
    // unsupported destination. Channels that own a transport the
    // command layer doesn't speak (HTTP for webhook, signed-link
    // minting for link, a bus publisher for bus) call
    // `send_invoice_render` instead and dispatch themselves.
    let scheme = scheme_of(&input.destination_uri);
    match scheme.as_str() {
        "file" | "stdout" => {}
        "bus" | "webhook" | "link" => {
            return Err(CoreError::NotImplemented(format!(
                "send_invoice: {scheme}:// dispatch must go through send_invoice_render + an adapter-owned transport"
            )));
        }
        other => {
            return Err(CoreError::Validation(format!(
                "send_invoice: unknown destination scheme `{other}`"
            )));
        }
    }

    // Idempotency replay: BEFORE mutating, sink-writing, or rendering
    // anything dispatch-heavy. The hit path re-loads + re-renders the
    // current invoice state (cheap) and returns with no side effects.
    if let Some(key) = input.idempotency_key.as_deref() {
        if let Some(replay) = try_replay(ctx, &input.invoice_id, key).await? {
            return Ok(replay);
        }
    }

    // Render + FSM-mutate shared with the render-only path.
    let prepared = prepare_send(
        ctx,
        &input.invoice_id,
        &input.actor,
        input.channel,
    )
    .await?;

    // Dispatch via the local sink (file:// or stdout).
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
            std::fs::write(&path, &prepared.pdf).map_err(|e| {
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
            sink.write_all(&prepared.pdf).map_err(|e| {
                CoreError::Validation(format!("writing to stdout sink: {e}"))
            })?;
            "stdout".to_string()
        }
        _ => unreachable!("scheme guard above"),
    };

    finalize_send(
        ctx,
        prepared,
        input.actor,
        input.channel,
        delivered_to,
        input.idempotency_key.as_deref(),
    )
    .await
}

/// Render an invoice + persist the FSM transition + history row, then
/// return the rendered bytes for an adapter to dispatch.
///
/// The destination string is recorded verbatim on the history row and
/// included in the emitted `inv.billing.invoice.sent` event. This is
/// the entry point channel adapters use for transports the command
/// layer doesn't speak natively — `webhook://...` (HTTP POST owned by
/// the api adapter), `link://` (signed-link minting), `bus://...`
/// (publisher owned by inv-bus).
#[tracing::instrument(skip_all, fields(
    invoice_id = %input.invoice_id,
    destination = %input.destination_uri
))]
pub async fn send_invoice_render(
    ctx: &CoreCtx,
    input: SendInvoiceRenderInput,
) -> Result<SendInvoiceOutput, CoreError> {
    input.validate()?;

    // Idempotency replay: same precondition as `send_invoice`. Adapter
    // dispatches (HTTP, link mint, bus publish) are NOT replayed — the
    // caller sees `idempotency_replay: true` and skips the dispatch
    // step. This matches the contract callers already follow for the
    // draft path.
    if let Some(key) = input.idempotency_key.as_deref() {
        if let Some(replay) = try_replay(ctx, &input.invoice_id, key).await? {
            return Ok(replay);
        }
    }

    let prepared = prepare_send(
        ctx,
        &input.invoice_id,
        &input.actor,
        input.channel,
    )
    .await?;
    let delivered_to = input.destination_uri.clone();
    finalize_send(
        ctx,
        prepared,
        input.actor,
        input.channel,
        delivered_to,
        input.idempotency_key.as_deref(),
    )
    .await
}

/// Intermediate output of [`prepare_send`] — everything the FSM mutate
/// path needs that doesn't depend on the destination scheme.
struct PreparedSend {
    invoice: Invoice,
    lines: Vec<InvoiceLine>,
    html: String,
    pdf: Vec<u8>,
    from_state: InvoiceState,
    to_state: InvoiceState,
    now: DateTime<Utc>,
}

/// Load invoice + lines + customer, FSM-resolve the target state,
/// render HTML + PDF. Performs NO writes — leaves state mutation +
/// history row + bus events to [`finalize_send`].
async fn prepare_send(
    ctx: &CoreCtx,
    invoice_id: &InvoiceId,
    _actor: &Actor,
    _channel: Channel,
) -> Result<PreparedSend, CoreError> {
    let inv_repo = InvoiceRepo::new(&ctx.db);
    let line_repo = InvoiceLineRepo::new(&ctx.db);
    let cust_repo = CustomerRepo::new(&ctx.db);

    let invoice = inv_repo
        .get(invoice_id)
        .await?
        .ok_or_else(|| CoreError::NotFound(format!("invoice {invoice_id}")))?;
    let lines = line_repo.list_for_invoice(invoice_id).await?;

    // FSM: Issued|Viewed → Sent. Sent → Sent is a "resend" — we permit
    // it explicitly here (it isn't legal in the FSM table). The history
    // row + event still fire so the audit trail records every send.
    let from_state = invoice.state;
    let to_state = match from_state {
        InvoiceState::Issued | InvoiceState::Viewed => {
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
                invoice.customer_id, invoice_id
            ))
        })?;

    let now = ctx.clock.now();

    let render_ctx = RenderContext::new(invoice.clone(), customer, lines.clone());
    let template_path = invoice.template_path.as_deref().map(std::path::Path::new);
    let html = render_html(template_path, &render_ctx).await?;
    let pdf = render_pdf(&html).await?;

    Ok(PreparedSend {
        invoice,
        lines,
        html,
        pdf,
        from_state,
        to_state,
        now,
    })
}

/// Commit the FSM mutation + history row + bus events. Called from
/// both `send_invoice` (after a local sink write) and
/// `send_invoice_render` (after the caller has captured the bytes).
///
/// When `idempotency_key` is set, also records the
/// `(invoice_id, idempotency_key)` row in `send_idempotency` inside the
/// same transaction. A future call with the same tuple short-circuits
/// before reaching this function (see [`try_replay`]).
async fn finalize_send(
    ctx: &CoreCtx,
    prepared: PreparedSend,
    actor: Actor,
    channel: Channel,
    delivered_to: String,
    idempotency_key: Option<&str>,
) -> Result<SendInvoiceOutput, CoreError> {
    let PreparedSend {
        mut invoice,
        lines,
        html,
        pdf,
        from_state,
        to_state,
        now,
    } = prepared;

    invoice.state = to_state;
    invoice.sent_at = Some(now);
    invoice.updated_at = now;

    let history_channel = HistoryChannel::from(channel);
    let history = InvoiceStateHistory {
        id: HistoryId::new(),
        invoice_id: invoice.id.clone(),
        from_state: Some(from_state),
        to_state,
        event: InvoiceEvent::Send.tag().to_string(),
        actor: Some(actor.audit_string()),
        channel: history_channel,
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
        if let Some(key) = idempotency_key {
            SendIdempotencyRepo::insert_in_tx(
                &mut tx,
                &SendIdempotencyRecord {
                    invoice_id: invoice.id.clone(),
                    idempotency_key: key.to_string(),
                    history_id: history.id.clone(),
                    delivered_to: delivered_to.clone(),
                    created_at: now,
                },
            )
            .await?;
        }
        tx.commit().await.map_err(inv_store::StoreError::from)?;
    }

    let emitted = build_sent_events(
        &invoice,
        from_state,
        to_state,
        &actor,
        history_channel,
        now,
        &delivered_to,
    );

    Ok(SendInvoiceOutput {
        invoice,
        lines,
        html,
        pdf,
        emitted_events: emitted,
        delivered_to,
        idempotency_replay: false,
    })
}

/// Idempotency replay path. Looks up the
/// `(invoice_id, idempotency_key)` tuple in `send_idempotency`; on a
/// hit, reloads the current invoice + lines + customer, re-renders
/// HTML + PDF deterministically from that state, and returns a
/// `SendInvoiceOutput` with `idempotency_replay: true` and an empty
/// `emitted_events` vector. Returns `Ok(None)` when no prior row
/// matches (caller proceeds with the normal send path).
async fn try_replay(
    ctx: &CoreCtx,
    invoice_id: &InvoiceId,
    idempotency_key: &str,
) -> Result<Option<SendInvoiceOutput>, CoreError> {
    let repo = SendIdempotencyRepo::new(&ctx.db);
    let Some(record) = repo.find(invoice_id, idempotency_key).await? else {
        return Ok(None);
    };

    let inv_repo = InvoiceRepo::new(&ctx.db);
    let line_repo = InvoiceLineRepo::new(&ctx.db);
    let cust_repo = CustomerRepo::new(&ctx.db);

    let invoice = inv_repo
        .get(invoice_id)
        .await?
        .ok_or_else(|| CoreError::NotFound(format!("invoice {invoice_id}")))?;
    let lines = line_repo.list_for_invoice(invoice_id).await?;
    let customer = cust_repo
        .get(&invoice.customer_id)
        .await?
        .ok_or_else(|| {
            CoreError::NotFound(format!(
                "customer {} (referenced by invoice {})",
                invoice.customer_id, invoice_id
            ))
        })?;

    let render_ctx = RenderContext::new(invoice.clone(), customer, lines.clone());
    let template_path = invoice.template_path.as_deref().map(std::path::Path::new);
    let html = render_html(template_path, &render_ctx).await?;
    let pdf = render_pdf(&html).await?;

    Ok(Some(SendInvoiceOutput {
        invoice,
        lines,
        html,
        pdf,
        emitted_events: Vec::new(),
        delivered_to: record.delivered_to,
        idempotency_replay: true,
    }))
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
