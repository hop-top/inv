//! `issue_invoice` command — `Draft → Issued`.
//!
//! Freezes the invoice: allocates a human number (`INV-YYYY-NNNN`),
//! resolves tax on every line, recomputes totals, renders HTML +
//! (stub) PDF, writes everything in a single sqlx transaction (in
//! intent — see the note in [`crate::draft`] about the repo-level
//! transaction surface we deliberately defer), and emits the
//! mechanic-event triplet (`.proposed` / `.transitioned` / `.entered`)
//! plus the domain-event (`.issued`).
//!
//! ## Number allocation
//!
//! `INV-YYYY-NNNN`, where YYYY is the issue year (UTC, from the
//! injected clock) and NNNN is `count(invoices issued this year) + 1`,
//! zero-padded to four digits. v1 best-effort — no global lock; under
//! contention two concurrent issues against an empty table can collide
//! and the `invoices.number UNIQUE` constraint will reject the second
//! save. T-0014 / T-0016 will harden this with a sequence table.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use sqlx::Row;

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
use inv_core::tax::{resolve_tax, NexusFigures};
use inv_store::repo::customer::CustomerRepo;
use inv_store::repo::history::InvoiceHistoryRepo;
use inv_store::repo::invoice::{InvoiceLineRepo, InvoiceRepo};

use crate::ctx::{Actor, Channel, CoreCtx};
use crate::draft::history_channel_str;
use crate::error::CoreError;
use crate::events::EmittedEvent;

/// Input for [`issue_invoice`].
#[derive(Debug, Clone)]
pub struct IssueInvoiceInput {
    /// Invoice to issue.
    pub invoice_id: InvoiceId,
    /// Caller-supplied dedupe key (currently advisory — issue is
    /// already idempotent because the FSM rejects `Issue` on
    /// non-`Draft` states).
    pub idempotency_key: Option<String>,
    /// Who triggered.
    pub actor: Actor,
    /// Channel the request arrived on.
    pub channel: Channel,
}

impl IssueInvoiceInput {
    /// Pure validation. Issue accepts everything the FSM accepts; the
    /// only check here is that `invoice_id` is well-formed (which the
    /// type already guarantees), so this is a no-op placeholder.
    pub fn validate(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

/// Output of [`issue_invoice`].
#[derive(Debug, Clone)]
pub struct IssueInvoiceOutput {
    /// Issued invoice (number assigned, tax frozen, totals recomputed).
    pub invoice: Invoice,
    /// Updated lines (tax_amount + line_total now populated).
    pub lines: Vec<InvoiceLine>,
    /// Rendered HTML body. PDF bytes are also persisted to
    /// [`CoreCtx::blob_store`] (when configured) and the resulting
    /// `blob://...` URI is recorded on `invoice.pdf_blob_ref`.
    pub html: String,
    /// PDF bytes (stub engine returns HTML verbatim at v1).
    pub pdf: Vec<u8>,
    /// Bus events the command would emit (T-0014 wires the real bus).
    pub emitted_events: Vec<EmittedEvent>,
}

/// Transition a draft to issued.
#[tracing::instrument(skip_all, fields(invoice_id = %input.invoice_id))]
pub async fn issue_invoice(
    ctx: &CoreCtx,
    input: IssueInvoiceInput,
) -> Result<IssueInvoiceOutput, CoreError> {
    input.validate()?;

    let inv_repo = InvoiceRepo::new(&ctx.db);
    let line_repo = InvoiceLineRepo::new(&ctx.db);
    let cust_repo = CustomerRepo::new(&ctx.db);

    // 1. Load draft.
    let mut invoice = inv_repo
        .get(&input.invoice_id)
        .await?
        .ok_or_else(|| CoreError::NotFound(format!("invoice {}", input.invoice_id)))?;
    let mut lines = line_repo.list_for_invoice(&input.invoice_id).await?;

    // 2. FSM gate: Draft -> Issued.
    let from_state = invoice.state;
    let event = InvoiceEvent::Issue;
    let to_state = next_state(from_state, &event)?;
    debug_assert_eq!(to_state, InvoiceState::Issued);

    // 3. Load customer (for buyer address used in tax resolution).
    let customer = cust_repo.get(&invoice.customer_id).await?.ok_or_else(|| {
        CoreError::NotFound(format!(
            "customer {} (referenced by invoice {})",
            invoice.customer_id, input.invoice_id
        ))
    })?;

    // 4. Resolve tax for every line and recompute totals.
    let now = ctx.clock.now();
    let nexus_figures = NexusFigures::default(); // v1: caller-side aggregation hooks land in T-0014.
    let mut subtotal = Decimal::ZERO;
    let mut tax_total = Decimal::ZERO;
    let mut nexus_review_any = false;
    for line in lines.iter_mut() {
        let resolved = resolve_tax(
            invoice.seller_jurisdiction,
            &customer.address,
            line,
            invoice.currency,
            &ctx.tax_table,
            &ctx.nexus,
            &nexus_figures,
            now,
        )?;
        let line_subtotal = invoice.currency.round(line.quantity * line.unit_price);
        line.tax_rate_ids = resolved.applied_rate_ids;
        line.tax_amount = resolved.amount;
        line.line_total = invoice.currency.round(line_subtotal + resolved.amount);
        subtotal += line_subtotal;
        tax_total += resolved.amount;
        nexus_review_any = nexus_review_any || resolved.nexus_review;
    }
    let subtotal = invoice.currency.round(subtotal);
    let tax_total = invoice.currency.round(tax_total);
    let total = invoice.currency.round(subtotal + tax_total);

    // 5. Allocate number (best-effort; design notes UNIQUE catches races).
    let year = now.year();
    let issued_count_this_year = count_invoices_issued_in_year(ctx, year).await?;
    let number = format!("INV-{year:04}-{:04}", issued_count_this_year + 1);

    // 6. Update invoice.
    invoice.state = InvoiceState::Issued;
    invoice.number = Some(number.clone());
    invoice.issued_at = Some(now);
    invoice.subtotal = subtotal;
    invoice.tax_total = tax_total;
    invoice.total = total;
    invoice.nexus_review = nexus_review_any;
    invoice.updated_at = now;

    // 7. Render HTML + (stub) PDF.
    let render_ctx = RenderContext::new(invoice.clone(), customer.clone(), lines.clone());
    let template_path = invoice.template_path.as_deref().map(std::path::Path::new);
    let html = render_html(template_path, &render_ctx).await?;
    let pdf = render_pdf(&html).await?;

    // 7a. Persist PDF to the blob store when one is configured. The
    //     returned `BlobRef` URI lands on `invoice.pdf_blob_ref` so
    //     downstream channels (send via `link://`, archive, etc.) can
    //     resolve back to the bytes. When no blob store is wired the
    //     command still succeeds — the operator just gets the bytes
    //     in the command output and `pdf_blob_ref` stays `None`.
    if let Some(blob_store) = ctx.blob_store.as_ref() {
        let blob_ref = blob_store.put(pdf.clone(), "application/pdf").await?;
        invoice.pdf_blob_ref = Some(blob_ref.as_str().to_string());
    }

    // 8. Persist mutation + audit row in a single sqlx transaction
    //    (design §3.5). InvoiceRepo::save_in_tx uses
    //    `INSERT ... ON CONFLICT DO UPDATE`, so the invoice row is
    //    updated in place — no CASCADE wipe of invoice_state_history.
    //    See T-0024.
    let history = InvoiceStateHistory {
        id: HistoryId::new(),
        invoice_id: invoice.id.clone(),
        from_state: Some(from_state),
        to_state,
        event: event.tag().to_string(),
        actor: Some(input.actor.audit_string()),
        channel: HistoryChannel::from(input.channel),
        bus_event_id: None,
        reason: None,
        occurred_at: now,
        published_at: None,
        metadata: BTreeMap::new(),
    };
    {
        let mut tx = ctx.db.begin().await.map_err(inv_store::StoreError::from)?;
        InvoiceRepo::save_in_tx(&mut tx, &invoice).await?;
        InvoiceLineRepo::replace_for_invoice_in_tx(&mut tx, &invoice.id, &lines).await?;
        InvoiceHistoryRepo::save_in_tx(&mut tx, &history).await?;
        tx.commit().await.map_err(inv_store::StoreError::from)?;
    }

    // 9. Build emitted events: mechanic-triplet + domain `.issued`.
    let emitted = build_issued_events(
        &invoice,
        from_state,
        to_state,
        &event,
        &input.actor,
        history.channel,
        now,
    );

    Ok(IssueInvoiceOutput {
        invoice,
        lines,
        html,
        pdf,
        emitted_events: emitted,
    })
}

/// Count invoices that have an `issued_at` falling within the given
/// year. Best-effort sequence source for `INV-YYYY-NNNN`.
async fn count_invoices_issued_in_year(ctx: &CoreCtx, year: i32) -> Result<u32, CoreError> {
    // Issued_at is rfc3339 in the column; a `BETWEEN` over rfc3339
    // strings sorts correctly because the prefix is the year.
    let from = format!("{year:04}-01-01T00:00:00Z");
    let to = format!("{:04}-01-01T00:00:00Z", year + 1);
    let row = sqlx::query(
        "SELECT COUNT(*) AS c FROM invoices \
         WHERE issued_at IS NOT NULL AND issued_at >= ? AND issued_at < ?",
    )
    .bind(from)
    .bind(to)
    .fetch_one(&ctx.db)
    .await
    .map_err(inv_store::StoreError::from)?;
    let c: i64 = row.try_get("c").map_err(inv_store::StoreError::from)?;
    Ok(c.max(0) as u32)
}

#[allow(clippy::too_many_arguments)]
fn build_issued_events(
    invoice: &Invoice,
    from: InvoiceState,
    to: InvoiceState,
    event: &InvoiceEvent,
    actor: &Actor,
    channel: HistoryChannel,
    now: DateTime<Utc>,
) -> Vec<EmittedEvent> {
    let actor_audit = actor.audit_string();

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
    let entered = InvoiceEntered::new(
        invoice.id.clone(),
        to,
        channel,
        Some(actor_audit.clone()),
        now,
    );

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
            "inv.billing.invoice.issued",
            json!({
                "invoice_id": invoice.id.to_string(),
                "number": invoice.number,
                "customer_id": invoice.customer_id.to_string(),
                "currency": invoice.currency.to_string(),
                "subtotal": invoice.subtotal.to_string(),
                "tax_total": invoice.tax_total.to_string(),
                "total": invoice.total.to_string(),
                "nexus_review": invoice.nexus_review,
                "actor": actor_audit,
                "channel": history_channel_str(channel),
            }),
            now,
        ),
    ]
}
