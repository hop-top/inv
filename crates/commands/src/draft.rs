//! `draft_invoice` command.
//!
//! Creates a new invoice in the [`Draft`](inv_core::domain::invoice::InvoiceState::Draft)
//! state, computes a pre-tax subtotal, writes the invoice + its lines +
//! a `drafted` row in `invoice_state_history`, all in a single sqlx
//! transaction. Tax is NOT resolved here — it's snapshotted at issue
//! time (T-0011 issue command) so drafts can be edited freely without
//! the rate landscape shifting under them.
//!
//! ## Idempotency
//!
//! If `idempotency_key` is supplied and an invoice with the same key
//! already exists, the existing invoice is returned unchanged (no
//! mutation, no new history row, no new events). This matches design
//! §3.5 and §4.4.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::json;

use inv_core::domain::ids::{CustomerId, HistoryId, InvoiceId, LineId};
use inv_core::domain::invoice::{
    HistoryChannel, Invoice, InvoiceLine, InvoiceState, InvoiceStateHistory, TaxCategory,
};
use inv_core::domain::jurisdiction::Jurisdiction;
use inv_core::domain::money::Currency;
use inv_store::repo::history::InvoiceHistoryRepo;
use inv_store::repo::invoice::{InvoiceFilter, InvoiceLineRepo, InvoiceRepo};

use crate::ctx::{Actor, Channel, CoreCtx};
use crate::error::CoreError;
use crate::events::EmittedEvent;

/// A single line on a draft.
///
/// Tax fields are not on the input — they're filled by the tax engine
/// at issue time. Description / quantity / price / category are the
/// caller-controlled fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftLineInput {
    /// Free-form description.
    pub description: String,
    /// Quantity (must be > 0).
    pub quantity: Decimal,
    /// Per-unit price (must be >= 0).
    pub unit_price: Decimal,
    /// Optional tax category override; defaults to `Standard`.
    pub tax_category: TaxCategory,
}

/// Input for [`draft_invoice`].
#[derive(Debug, Clone)]
pub struct DraftInvoiceInput {
    /// Bill-to customer (must already exist in the customers table).
    pub customer_id: CustomerId,
    /// Seller jurisdiction (drives tax resolution at issue).
    pub seller_jurisdiction: Jurisdiction,
    /// Invoice currency (every line is in this currency).
    pub currency: Currency,
    /// Lines (at least one required).
    pub lines: Vec<DraftLineInput>,
    /// Caller-supplied dedupe key. If set and a previous invoice has
    /// the same key, that invoice is returned unchanged.
    pub idempotency_key: Option<String>,
    /// Who triggered.
    pub actor: Actor,
    /// Channel the request arrived on.
    pub channel: Channel,
    /// Optional payment due date.
    pub due_at: Option<DateTime<Utc>>,
    /// Optional per-invoice template override (filesystem path).
    pub template_path: Option<String>,
}

impl DraftInvoiceInput {
    /// Pure validation step (see design §3.5 — runs first in every command).
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.lines.is_empty() {
            return Err(CoreError::Validation(
                "draft_invoice requires at least one line".into(),
            ));
        }
        // v1 supports USD/CAD/DZD only (design §2). Reject other codes
        // up front so the audit row + tax engine never see an
        // unsupported currency.
        let cur = self.currency.as_str();
        if !matches!(cur, "USD" | "CAD" | "DZD") {
            return Err(CoreError::Validation(format!(
                "currency {cur} not supported at v1 (use USD, CAD, or DZD)"
            )));
        }
        for (i, line) in self.lines.iter().enumerate() {
            if line.description.trim().is_empty() {
                return Err(CoreError::Validation(format!(
                    "line {i}: description must not be empty"
                )));
            }
            if line.quantity <= Decimal::ZERO {
                return Err(CoreError::Validation(format!(
                    "line {i}: quantity must be > 0 (got {})",
                    line.quantity
                )));
            }
            if line.unit_price < Decimal::ZERO {
                return Err(CoreError::Validation(format!(
                    "line {i}: unit_price must be >= 0 (got {})",
                    line.unit_price
                )));
            }
        }
        Ok(())
    }
}

/// Output of [`draft_invoice`].
#[derive(Debug, Clone)]
pub struct DraftInvoiceOutput {
    /// The persisted invoice.
    pub invoice: Invoice,
    /// The persisted lines (sorted by position).
    pub lines: Vec<InvoiceLine>,
    /// Bus events the command would emit (T-0014 wires the real bus).
    pub emitted_events: Vec<EmittedEvent>,
    /// True if the command short-circuited on an idempotency-key hit
    /// (no mutation, no new history row, no events).
    pub idempotency_replay: bool,
}

/// Create a new draft invoice.
#[tracing::instrument(skip_all, fields(customer_id = %input.customer_id))]
pub async fn draft_invoice(
    ctx: &CoreCtx,
    input: DraftInvoiceInput,
) -> Result<DraftInvoiceOutput, CoreError> {
    // 1. Validation (design §3.5: runs first).
    input.validate()?;

    // 2. Idempotency check (design §3.5: BEFORE mutating).
    if let Some(key) = input.idempotency_key.as_deref() {
        if let Some(existing) = find_invoice_by_idempotency_key(ctx, key).await? {
            let lines = InvoiceLineRepo::new(&ctx.db)
                .list_for_invoice(&existing.id)
                .await?;
            return Ok(DraftInvoiceOutput {
                invoice: existing,
                lines,
                emitted_events: Vec::new(),
                idempotency_replay: true,
            });
        }
    }

    // 3. Build the invoice + lines (pre-tax; tax frozen at issue).
    let now = ctx.clock.now();
    let invoice_id = InvoiceId::new();

    let mut lines: Vec<InvoiceLine> = Vec::with_capacity(input.lines.len());
    let mut subtotal = Decimal::ZERO;
    for (pos, l) in input.lines.iter().enumerate() {
        let line_subtotal = l.quantity * l.unit_price;
        let rounded = input.currency.round(line_subtotal);
        subtotal += rounded;
        lines.push(InvoiceLine {
            id: LineId::new(),
            invoice_id: invoice_id.clone(),
            position: pos as u32,
            description: l.description.clone(),
            quantity: l.quantity,
            unit_price: l.unit_price,
            tax_rate_ids: Vec::new(),
            tax_category: l.tax_category,
            tax_amount: Decimal::ZERO,
            line_total: rounded,
            metadata: BTreeMap::new(),
        });
    }
    let subtotal = input.currency.round(subtotal);

    let invoice = Invoice {
        id: invoice_id.clone(),
        number: None,
        customer_id: input.customer_id.clone(),
        seller_jurisdiction: input.seller_jurisdiction,
        currency: input.currency,
        state: InvoiceState::Draft,
        issued_at: None,
        due_at: input.due_at,
        sent_at: None,
        viewed_at: None,
        paid_at: None,
        voided_at: None,
        subtotal,
        tax_total: Decimal::ZERO,
        total: subtotal,
        amount_paid: Decimal::ZERO,
        schedule_id: None,
        template_path: input.template_path.clone(),
        pdf_blob_ref: None,
        idempotency_key: input.idempotency_key.clone(),
        nexus_review: false,
        metadata: BTreeMap::new(),
        created_at: now,
        updated_at: now,
    };

    // 4. Persist + audit row in a single transaction (design §3.5).
    let history = InvoiceStateHistory {
        id: HistoryId::new(),
        invoice_id: invoice_id.clone(),
        from_state: None,
        to_state: InvoiceState::Draft,
        event: "draft".into(),
        actor: Some(input.actor.audit_string()),
        channel: HistoryChannel::from(input.channel),
        bus_event_id: None,
        reason: None,
        occurred_at: now,
        published_at: None,
        metadata: BTreeMap::new(),
    };

    // Single sqlx transaction encompassing invoice + lines + history
    // row (design §3.5). Each repo exposes a `save_in_tx` variant that
    // accepts a borrowed `&mut Transaction`. See T-0024.
    {
        let mut tx = ctx.db.begin().await.map_err(inv_store::StoreError::from)?;
        InvoiceRepo::save_in_tx(&mut tx, &invoice).await?;
        InvoiceLineRepo::replace_for_invoice_in_tx(&mut tx, &invoice_id, &lines).await?;
        InvoiceHistoryRepo::save_in_tx(&mut tx, &history).await?;
        tx.commit().await.map_err(inv_store::StoreError::from)?;
    }

    // 5. Construct the emitted-events list.
    let drafted_event = EmittedEvent::new(
        "inv.billing.invoice.drafted",
        json!({
            "invoice_id": invoice_id.to_string(),
            "customer_id": invoice.customer_id.to_string(),
            "currency": invoice.currency.to_string(),
            "subtotal": invoice.subtotal.to_string(),
            "total": invoice.total.to_string(),
            "actor": input.actor.audit_string(),
            "channel": history_channel_str(history.channel),
        }),
        now,
    );

    Ok(DraftInvoiceOutput {
        invoice,
        lines,
        emitted_events: vec![drafted_event],
        idempotency_replay: false,
    })
}

/// Look up an existing invoice by its idempotency key.
///
/// Linear scan of invoices for v1 — the index lives on the column at
/// schema level (UNIQUE) but the repo doesn't expose a direct lookup.
/// At typical v1 throughput (dozens of drafts/day per operator) the
/// scan is bounded by `list()`'s default ordering and is dominated by
/// SQL planner overhead either way.
async fn find_invoice_by_idempotency_key(
    ctx: &CoreCtx,
    key: &str,
) -> Result<Option<Invoice>, CoreError> {
    let inv_repo = InvoiceRepo::new(&ctx.db);
    let list = inv_repo
        .list(&InvoiceFilter {
            customer_id: None,
            state: None,
            limit: None,
            offset: None,
        })
        .await?;
    Ok(list.into_iter().find(|i| {
        i.idempotency_key
            .as_deref()
            .is_some_and(|k| k == key)
    }))
}

/// Stringify the [`HistoryChannel`] for the event payload.
pub(crate) fn history_channel_str(c: HistoryChannel) -> &'static str {
    match c {
        HistoryChannel::Cli => "cli",
        HistoryChannel::Api => "api",
        HistoryChannel::Ws => "ws",
        HistoryChannel::Mcp => "mcp",
        HistoryChannel::Bus => "bus",
    }
}
