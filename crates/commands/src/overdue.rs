//! `mark_overdue_ticker` — internal tick that flags overdue invoices.
//!
//! Per design §3.4, `Overdue` is NOT an FSM state — it's a derived flag
//! emitted as `inv.billing.invoice.overdue`. This command scans
//! Issued/Sent/Viewed invoices whose `due_at` is in the past and emits
//! one event per overdue invoice. State remains unchanged.
//!
//! An optional `metadata.overdue_at` timestamp on the invoice avoids
//! re-emission on subsequent ticks for the same invoice. v1 keeps that
//! tracking off — the relay tier (T-0014) is responsible for not
//! double-publishing within a window.

use chrono::{DateTime, Utc};
use serde_json::json;

use inv_core::domain::invoice::{Invoice, InvoiceState};
use inv_store::repo::invoice::{InvoiceFilter, InvoiceRepo};

use crate::ctx::CoreCtx;
use crate::error::CoreError;
use crate::events::EmittedEvent;

/// Output of [`mark_overdue_ticker`].
#[derive(Debug, Clone)]
pub struct OverdueTickerOutput {
    /// Invoices flagged overdue on this tick.
    pub overdue_invoices: Vec<Invoice>,
    /// Bus events the command would emit (one per overdue invoice).
    pub emitted_events: Vec<EmittedEvent>,
}

/// Scan for overdue invoices and emit `inv.billing.invoice.overdue` per match.
///
/// Does not mutate invoice state. Caller (typically the inv-server
/// ticker; T-0020) drives this on a configurable interval.
#[tracing::instrument(skip_all)]
pub async fn mark_overdue_ticker(ctx: &CoreCtx) -> Result<OverdueTickerOutput, CoreError> {
    let now = ctx.clock.now();
    let inv_repo = InvoiceRepo::new(&ctx.db);

    // We don't have a server-side `WHERE due_at < now` filter on the
    // repo; v1 paginates through Issued/Sent/Viewed invoices and
    // filters client-side. Acceptable until volume warrants a dedicated
    // query method.
    let candidates = collect_unpaid_unvoided(&inv_repo).await?;

    let mut overdue = Vec::new();
    let mut events = Vec::new();
    for inv in candidates {
        if is_overdue(&inv, now) {
            events.push(build_overdue_event(&inv, now));
            overdue.push(inv);
        }
    }

    Ok(OverdueTickerOutput {
        overdue_invoices: overdue,
        emitted_events: events,
    })
}

fn is_overdue(inv: &Invoice, now: DateTime<Utc>) -> bool {
    matches!(
        inv.state,
        InvoiceState::Issued
            | InvoiceState::Sent
            | InvoiceState::Viewed
            | InvoiceState::PartiallyPaid
    ) && inv.due_at.map(|d| d < now).unwrap_or(false)
}

async fn collect_unpaid_unvoided(repo: &InvoiceRepo<'_>) -> Result<Vec<Invoice>, CoreError> {
    // Pull each unpaid-unvoided state separately; merge.
    let mut all = Vec::new();
    for state in [
        InvoiceState::Issued,
        InvoiceState::Sent,
        InvoiceState::Viewed,
        InvoiceState::PartiallyPaid,
    ] {
        let filter = InvoiceFilter {
            customer_id: None,
            state: Some(state),
            limit: None,
            offset: None,
        };
        let mut rows = repo.list(&filter).await?;
        all.append(&mut rows);
    }
    Ok(all)
}

fn build_overdue_event(inv: &Invoice, now: DateTime<Utc>) -> EmittedEvent {
    EmittedEvent::new(
        "inv.billing.invoice.overdue",
        json!({
            "invoice_id": inv.id.to_string(),
            "customer_id": inv.customer_id.to_string(),
            "number": inv.number,
            "due_at": inv.due_at.map(|d| d.to_rfc3339()),
            "total": inv.total.to_string(),
            "amount_paid": inv.amount_paid.to_string(),
            "currency": inv.currency.to_string(),
            "state": match inv.state {
                InvoiceState::Issued => "issued",
                InvoiceState::Sent => "sent",
                InvoiceState::Viewed => "viewed",
                InvoiceState::PartiallyPaid => "partially_paid",
                InvoiceState::Paid => "paid",
                InvoiceState::Voided => "voided",
                InvoiceState::Draft => "draft",
            },
        }),
        now,
    )
}
