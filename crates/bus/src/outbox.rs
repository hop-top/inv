//! Transactional outbox relay.
//!
//! Polls `invoice_state_history` + `credit_note_state_history` for rows
//! with `published_at IS NULL`, rebuilds the bus event set for each
//! row, publishes via the caller's [`Publisher`], and marks the row
//! published. Subscribers must tolerate at-least-once (design §4.4).
//!
//! ## History-row → event-set mapping
//!
//! Each history row produces 0 or more bus events. The mapping is:
//!
//! - **Initial row** (`from_state IS NULL`, e.g. an invoice's "draft"
//!   or a credit note's "draft" row): emit only the domain event
//!   (`inv.billing.invoice.drafted` / `…creditnote.drafted`). There is
//!   no FSM transition — the object was created in its initial state —
//!   so no `.proposed` / `.transitioned` / `.entered` triplet.
//! - **Transition row** (`from_state IS SOME`): emit the mechanic triplet
//!   in order `.proposed` → `.transitioned` → `.entered`, then the
//!   matching domain event (`…issued`, `…sent`, `…viewed`, `…paid` or
//!   `…partially_paid`, `…voided`, `…creditnote.issued`).
//! - **Self-edge** (`from_state == to_state`): currently only `Sent +
//!   remind` on invoices; the commands layer doesn't write a history
//!   row for self-edges, so the outbox never sees one. Defensive code
//!   in the mapper still handles the case by skipping the mechanic
//!   triplet (the FSM didn't move).
//!
//! Domain-topic resolution looks at the history row's `event` field
//! (`"draft"`, `"issue"`, `"send"`, `"view"`, `"pay"`, `"void"` for
//! invoices; `"draft"`, `"issue"` for credit notes) AND `to_state` —
//! the "pay" event splits between `…paid` and `…partially_paid`
//! based on the resulting state.

use serde_json::{json, Value};
use tracing::{debug, warn};

use inv_core::domain::creditnote::{CreditNoteState, CreditNoteStateHistory};
use inv_core::domain::invoice::{HistoryChannel, InvoiceState, InvoiceStateHistory};
use inv_core::state::creditnote::{
    CreditNoteEntered, CreditNoteEvent, CreditNoteProposed, CreditNoteTransitioned,
};
use inv_core::state::events::{InvoiceEntered, InvoiceProposed, InvoiceTransitioned};
use inv_core::state::transitions::InvoiceEvent;
use inv_store::repo::credit_note::CreditNoteHistoryRepo;
use inv_store::repo::history::InvoiceHistoryRepo;
use inv_store::Pool;

use crate::error::RelayError;
use crate::events::{
    TOPIC_CREDITNOTE_DRAFTED, TOPIC_CREDITNOTE_ISSUED, TOPIC_INVOICE_DRAFTED,
    TOPIC_INVOICE_ISSUED, TOPIC_INVOICE_PAID, TOPIC_INVOICE_PARTIALLY_PAID,
    TOPIC_INVOICE_SENT, TOPIC_INVOICE_VIEWED, TOPIC_INVOICE_VOIDED,
};
use crate::publisher::Publisher;

/// Per-run outcome of [`run_outbox_relay`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OutboxStats {
    /// Number of pending history rows that were processed and marked
    /// published in this run (sum across invoice + credit-note tables).
    pub rows_processed: usize,
    /// Number of bus events published (= rows × per-row event count).
    pub events_published: usize,
}

/// Single pass of the relay.
///
/// Reads pending rows from both history tables, publishes every event,
/// marks each row. Returns the per-run stats. A second call after a
/// successful first call is a no-op (every row is now marked published).
///
/// `limit` caps the number of rows scanned per table per call so a
/// large backlog doesn't monopolise a single tick. Callers typically
/// invoke this in a loop with backoff.
pub async fn run_outbox_relay(
    pool: &Pool,
    publisher: &dyn Publisher,
    limit: i64,
) -> Result<OutboxStats, RelayError> {
    let mut stats = OutboxStats::default();

    // -- invoice rows ----------------------------------------------------
    let inv_repo = InvoiceHistoryRepo::new(pool);
    let pending_inv = inv_repo.pending_outbox(limit).await?;
    for row in pending_inv {
        let events = invoice_row_to_events(&row);
        for (topic, payload) in events {
            publisher
                .publish(&topic, payload, row.occurred_at)
                .await?;
            stats.events_published += 1;
        }
        inv_repo.mark_published(&row.id).await?;
        stats.rows_processed += 1;
    }

    // -- credit-note rows ------------------------------------------------
    let cn_repo = CreditNoteHistoryRepo::new(pool);
    let pending_cn = cn_repo.pending_outbox(limit).await?;
    for row in pending_cn {
        let events = credit_note_row_to_events(&row);
        for (topic, payload) in events {
            publisher
                .publish(&topic, payload, row.occurred_at)
                .await?;
            stats.events_published += 1;
        }
        cn_repo.mark_published(&row.id).await?;
        stats.rows_processed += 1;
    }

    Ok(stats)
}

// =============================================================================
// Invoice row -> event set
// =============================================================================

fn invoice_row_to_events(row: &InvoiceStateHistory) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = Vec::new();

    // Self-edges don't write history rows under v1; defensive skip.
    if row.from_state == Some(row.to_state) {
        debug!(
            target: "inv_bus::outbox",
            history_id = %row.id,
            "skip mechanic triplet for self-edge history row"
        );
    } else if row.from_state.is_some() {
        // Reconstruct the FSM event from the audit tag + to_state. The
        // mapping is exhaustive for v1 (see design §3.4); unknown tags
        // skip the triplet and only emit the domain event below.
        if let Some(ev) = reconstruct_invoice_event(&row.event, row.to_state) {
            let from = row.from_state.unwrap_or(row.to_state);
            let proposed = InvoiceProposed::new(
                row.invoice_id.clone(),
                from,
                row.to_state,
                ev.clone(),
                row.actor.clone(),
                row.channel,
                row.occurred_at,
            );
            let transitioned = InvoiceTransitioned::new(
                row.invoice_id.clone(),
                from,
                row.to_state,
                ev.clone(),
                row.actor.clone(),
                row.channel,
                row.occurred_at,
            );
            let entered = InvoiceEntered::new(
                row.invoice_id.clone(),
                row.to_state,
                row.channel,
                row.actor.clone(),
                row.occurred_at,
            );

            out.push((
                inv_core::state::events::TOPIC_PROPOSED.to_string(),
                serde_json::to_value(&proposed).unwrap_or_else(|_| json!({})),
            ));
            out.push((
                inv_core::state::events::TOPIC_TRANSITIONED.to_string(),
                serde_json::to_value(&transitioned).unwrap_or_else(|_| json!({})),
            ));
            out.push((
                inv_core::state::events::TOPIC_ENTERED.to_string(),
                serde_json::to_value(&entered).unwrap_or_else(|_| json!({})),
            ));
        } else {
            warn!(
                target: "inv_bus::outbox",
                event = %row.event,
                to_state = ?row.to_state,
                "unknown invoice history event tag; skipping mechanic triplet"
            );
        }
    }

    // Domain event for this row.
    if let Some(topic) = invoice_domain_topic(&row.event, row.to_state) {
        out.push((topic.to_string(), invoice_domain_payload(row)));
    } else {
        warn!(
            target: "inv_bus::outbox",
            event = %row.event,
            to_state = ?row.to_state,
            "no domain topic mapping for invoice history row"
        );
    }

    out
}

/// Reconstruct an [`InvoiceEvent`] from the audit-tag string + the row's
/// `to_state`. Returns `None` for tags that don't correspond to an FSM
/// event (e.g. the initial `"draft"` row, which has `from_state IS NULL`
/// and is handled by the caller).
fn reconstruct_invoice_event(tag: &str, to_state: InvoiceState) -> Option<InvoiceEvent> {
    use rust_decimal::Decimal;
    match tag {
        "issue" => Some(InvoiceEvent::Issue),
        "send" => Some(InvoiceEvent::Send),
        "remind" => Some(InvoiceEvent::Remind),
        "view" => Some(InvoiceEvent::View),
        "pay" => {
            // The Decimal fields on `Pay` are used by the FSM table to
            // classify (full vs partial); for replay purposes the
            // verdict is already encoded in `to_state`. Use sentinel
            // values that re-classify correctly: amount=1, total=1
            // for Paid (full), amount=1, total=2 for PartiallyPaid.
            match to_state {
                InvoiceState::Paid => Some(InvoiceEvent::Pay {
                    amount_paid: Decimal::ONE,
                    total: Decimal::ONE,
                }),
                InvoiceState::PartiallyPaid => Some(InvoiceEvent::Pay {
                    amount_paid: Decimal::ONE,
                    total: Decimal::new(2, 0),
                }),
                _ => None,
            }
        }
        "void" => Some(InvoiceEvent::Void),
        // "draft" is the initial-row tag with from_state IS NULL and is
        // never an FSM transition event; caller skips the triplet.
        _ => None,
    }
}

fn invoice_domain_topic(tag: &str, to_state: InvoiceState) -> Option<&'static str> {
    match (tag, to_state) {
        ("draft", InvoiceState::Draft) => Some(TOPIC_INVOICE_DRAFTED),
        ("issue", InvoiceState::Issued) => Some(TOPIC_INVOICE_ISSUED),
        ("send", InvoiceState::Sent) => Some(TOPIC_INVOICE_SENT),
        ("view", InvoiceState::Viewed) => Some(TOPIC_INVOICE_VIEWED),
        ("pay", InvoiceState::Paid) => Some(TOPIC_INVOICE_PAID),
        ("pay", InvoiceState::PartiallyPaid) => Some(TOPIC_INVOICE_PARTIALLY_PAID),
        ("void", InvoiceState::Voided) => Some(TOPIC_INVOICE_VOIDED),
        _ => None,
    }
}

fn invoice_domain_payload(row: &InvoiceStateHistory) -> Value {
    // Replayed payload — the persisted row has audit-grade fields only.
    // Subscribers needing the full invoice body (totals, customer, …)
    // re-fetch from the repos. The commands layer's emit-time payloads
    // are richer; here we ship the audit envelope that's always
    // reconstructable.
    json!({
        "invoice_id": row.invoice_id.to_string(),
        "from_state": row.from_state.map(invoice_state_str),
        "to_state": invoice_state_str(row.to_state),
        "event": row.event,
        "actor": row.actor,
        "channel": channel_str(row.channel),
        "occurred_at": row.occurred_at,
        "reason": row.reason,
    })
}

// =============================================================================
// Credit-note row -> event set
// =============================================================================

fn credit_note_row_to_events(row: &CreditNoteStateHistory) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = Vec::new();

    if row.from_state.is_some() && row.from_state != Some(row.to_state) {
        let ev = CreditNoteEvent::Issue; // Only event the CN FSM has.
        let from = row.from_state.unwrap_or(row.to_state);
        let proposed = CreditNoteProposed::new(
            row.credit_note_id.clone(),
            from,
            row.to_state,
            ev.clone(),
            row.actor.clone(),
            row.channel,
            row.occurred_at,
        );
        let transitioned = CreditNoteTransitioned::new(
            row.credit_note_id.clone(),
            from,
            row.to_state,
            ev.clone(),
            row.actor.clone(),
            row.channel,
            row.occurred_at,
        );
        let entered = CreditNoteEntered::new(
            row.credit_note_id.clone(),
            row.to_state,
            row.channel,
            row.actor.clone(),
            row.occurred_at,
        );
        out.push((
            inv_core::state::creditnote::TOPIC_PROPOSED.to_string(),
            serde_json::to_value(&proposed).unwrap_or_else(|_| json!({})),
        ));
        out.push((
            inv_core::state::creditnote::TOPIC_TRANSITIONED.to_string(),
            serde_json::to_value(&transitioned).unwrap_or_else(|_| json!({})),
        ));
        out.push((
            inv_core::state::creditnote::TOPIC_ENTERED.to_string(),
            serde_json::to_value(&entered).unwrap_or_else(|_| json!({})),
        ));
    }

    if let Some(topic) = credit_note_domain_topic(&row.event, row.to_state) {
        out.push((topic.to_string(), credit_note_domain_payload(row)));
    } else {
        warn!(
            target: "inv_bus::outbox",
            event = %row.event,
            to_state = ?row.to_state,
            "no domain topic mapping for credit-note history row"
        );
    }

    out
}

fn credit_note_domain_topic(tag: &str, to_state: CreditNoteState) -> Option<&'static str> {
    match (tag, to_state) {
        ("draft", CreditNoteState::Draft) => Some(TOPIC_CREDITNOTE_DRAFTED),
        ("issue", CreditNoteState::Issued) => Some(TOPIC_CREDITNOTE_ISSUED),
        _ => None,
    }
}

fn credit_note_domain_payload(row: &CreditNoteStateHistory) -> Value {
    json!({
        "credit_note_id": row.credit_note_id.to_string(),
        "from_state": row.from_state.map(credit_note_state_str),
        "to_state": credit_note_state_str(row.to_state),
        "event": row.event,
        "actor": row.actor,
        "channel": channel_str(row.channel),
        "occurred_at": row.occurred_at,
    })
}

// =============================================================================
// Stringify helpers (kept local to keep store internals encapsulated).
// =============================================================================

fn invoice_state_str(s: InvoiceState) -> &'static str {
    match s {
        InvoiceState::Draft => "draft",
        InvoiceState::Issued => "issued",
        InvoiceState::Sent => "sent",
        InvoiceState::Viewed => "viewed",
        InvoiceState::PartiallyPaid => "partially_paid",
        InvoiceState::Paid => "paid",
        InvoiceState::Voided => "voided",
    }
}

fn credit_note_state_str(s: CreditNoteState) -> &'static str {
    match s {
        CreditNoteState::Draft => "draft",
        CreditNoteState::Issued => "issued",
    }
}

fn channel_str(c: HistoryChannel) -> &'static str {
    match c {
        HistoryChannel::Cli => "cli",
        HistoryChannel::Api => "api",
        HistoryChannel::Ws => "ws",
        HistoryChannel::Mcp => "mcp",
        HistoryChannel::Bus => "bus",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use inv_core::domain::ids::{HistoryId, InvoiceId};
    use std::collections::BTreeMap;

    fn invoice_row(
        event: &str,
        from: Option<InvoiceState>,
        to: InvoiceState,
    ) -> InvoiceStateHistory {
        InvoiceStateHistory {
            id: HistoryId::new(),
            invoice_id: InvoiceId::new(),
            from_state: from,
            to_state: to,
            event: event.into(),
            actor: Some("jad".into()),
            channel: HistoryChannel::Cli,
            bus_event_id: None,
            reason: None,
            occurred_at: Utc::now(),
            published_at: None,
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn mapper_initial_draft_emits_domain_only() {
        let row = invoice_row("draft", None, InvoiceState::Draft);
        let events = invoice_row_to_events(&row);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, TOPIC_INVOICE_DRAFTED);
    }

    #[test]
    fn mapper_issue_transition_emits_triplet_plus_domain() {
        let row = invoice_row("issue", Some(InvoiceState::Draft), InvoiceState::Issued);
        let events = invoice_row_to_events(&row);
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].0, inv_core::state::events::TOPIC_PROPOSED);
        assert_eq!(events[1].0, inv_core::state::events::TOPIC_TRANSITIONED);
        assert_eq!(events[2].0, inv_core::state::events::TOPIC_ENTERED);
        assert_eq!(events[3].0, TOPIC_INVOICE_ISSUED);
    }

    #[test]
    fn mapper_pay_splits_paid_vs_partially_paid() {
        let row_paid = invoice_row("pay", Some(InvoiceState::Sent), InvoiceState::Paid);
        let row_partial = invoice_row(
            "pay",
            Some(InvoiceState::Sent),
            InvoiceState::PartiallyPaid,
        );
        let ev_paid = invoice_row_to_events(&row_paid);
        let ev_partial = invoice_row_to_events(&row_partial);
        assert_eq!(ev_paid.last().unwrap().0, TOPIC_INVOICE_PAID);
        assert_eq!(ev_partial.last().unwrap().0, TOPIC_INVOICE_PARTIALLY_PAID);
    }
}
