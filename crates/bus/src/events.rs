//! Typed payload structs for every domain event in design §4.2.
//!
//! The mechanic-event payloads (`InvoiceProposed`, `InvoiceTransitioned`,
//! `InvoiceEntered` + the credit-note triplet) live in `inv-core::state`
//! and are re-exported from [`crate`]'s root — not duplicated here.
//!
//! All payloads round-trip through `serde_json` (Serialize + Deserialize).
//! Field shapes mirror the JSON bodies that `inv-commands` builds inline
//! today (see `crates/commands/src/{draft,issue,send,pay,void,credit,
//! overdue,reminder,schedule}.rs`); when this crate publishes from the
//! outbox it (re)builds the same shape from the persisted history row.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

// =============================================================================
// Topic constants — design §4.2 (domain event surface)
// =============================================================================

/// `inv.billing.invoice.drafted`
pub const TOPIC_INVOICE_DRAFTED: &str = "inv.billing.invoice.drafted";
/// `inv.billing.invoice.issued`
pub const TOPIC_INVOICE_ISSUED: &str = "inv.billing.invoice.issued";
/// `inv.billing.invoice.sent`
pub const TOPIC_INVOICE_SENT: &str = "inv.billing.invoice.sent";
/// `inv.billing.invoice.viewed`
pub const TOPIC_INVOICE_VIEWED: &str = "inv.billing.invoice.viewed";
/// `inv.billing.invoice.paid`
pub const TOPIC_INVOICE_PAID: &str = "inv.billing.invoice.paid";
/// `inv.billing.invoice.partially_paid`
pub const TOPIC_INVOICE_PARTIALLY_PAID: &str = "inv.billing.invoice.partially_paid";
/// `inv.billing.invoice.overdue`
pub const TOPIC_INVOICE_OVERDUE: &str = "inv.billing.invoice.overdue";
/// `inv.billing.invoice.voided`
pub const TOPIC_INVOICE_VOIDED: &str = "inv.billing.invoice.voided";

/// `inv.billing.creditnote.drafted`
pub const TOPIC_CREDITNOTE_DRAFTED: &str = "inv.billing.creditnote.drafted";
/// `inv.billing.creditnote.issued`
pub const TOPIC_CREDITNOTE_ISSUED: &str = "inv.billing.creditnote.issued";

/// `inv.billing.reminder.scheduled`
pub const TOPIC_REMINDER_SCHEDULED: &str = "inv.billing.reminder.scheduled";
/// `inv.billing.reminder.sent`
pub const TOPIC_REMINDER_SENT: &str = "inv.billing.reminder.sent";
/// `inv.billing.reminder.cancelled`
pub const TOPIC_REMINDER_CANCELLED: &str = "inv.billing.reminder.cancelled";

/// `inv.billing.schedule.created`
pub const TOPIC_SCHEDULE_CREATED: &str = "inv.billing.schedule.created";
/// `inv.billing.schedule.paused`
pub const TOPIC_SCHEDULE_PAUSED: &str = "inv.billing.schedule.paused";
/// `inv.billing.schedule.cancelled`
pub const TOPIC_SCHEDULE_CANCELLED: &str = "inv.billing.schedule.cancelled";

// =============================================================================
// Invoice domain payloads
// =============================================================================

/// Payload for [`TOPIC_INVOICE_DRAFTED`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceDrafted {
    /// Newly-created invoice id.
    pub invoice_id: String,
    /// Bill-to customer id.
    pub customer_id: String,
    /// Invoice currency (ISO 4217).
    pub currency: String,
    /// Pre-tax subtotal (Decimal as string).
    pub subtotal: String,
    /// Total (= subtotal at draft time; tax frozen at issue).
    pub total: String,
    /// Audit string for the triggering actor.
    pub actor: String,
    /// Channel the command came in on (`cli`, `api`, `ws`, `mcp`, `bus`).
    pub channel: String,
}

/// Payload for [`TOPIC_INVOICE_ISSUED`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceIssued {
    /// Invoice id.
    pub invoice_id: String,
    /// Assigned invoice number (e.g. `INV-2026-0001`).
    pub number: Option<String>,
    /// Bill-to customer id.
    pub customer_id: String,
    /// Currency (ISO 4217).
    pub currency: String,
    /// Subtotal pre-tax (Decimal as string).
    pub subtotal: String,
    /// Tax total (Decimal as string).
    pub tax_total: String,
    /// Total = subtotal + tax (Decimal as string).
    pub total: String,
    /// Operator-review flag (US nexus undetermined).
    pub nexus_review: bool,
    /// Audit string for the triggering actor.
    pub actor: String,
    /// Channel.
    pub channel: String,
}

/// Payload for [`TOPIC_INVOICE_SENT`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceSent {
    /// Invoice id.
    pub invoice_id: String,
    /// Audit string for the triggering actor.
    pub actor: String,
    /// Channel.
    pub channel: String,
}

/// Payload for [`TOPIC_INVOICE_VIEWED`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceViewed {
    /// Invoice id.
    pub invoice_id: String,
    /// Audit string for the triggering actor.
    pub actor: String,
    /// Channel.
    pub channel: String,
}

/// Payload for [`TOPIC_INVOICE_PAID`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoicePaid {
    /// Invoice id.
    pub invoice_id: String,
    /// Amount paid in this transaction (Decimal as string).
    pub amount_paid: String,
    /// Cumulative amount paid so far (Decimal as string).
    pub cumulative_paid: String,
    /// Invoice total (Decimal as string).
    pub total: String,
    /// Audit actor.
    pub actor: String,
    /// Channel.
    pub channel: String,
}

/// Payload for [`TOPIC_INVOICE_PARTIALLY_PAID`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoicePartiallyPaid {
    /// Invoice id.
    pub invoice_id: String,
    /// Amount paid in this transaction (Decimal as string).
    pub amount_paid: String,
    /// Cumulative amount paid so far (Decimal as string).
    pub cumulative_paid: String,
    /// Invoice total (Decimal as string).
    pub total: String,
    /// Audit actor.
    pub actor: String,
    /// Channel.
    pub channel: String,
}

/// Payload for [`TOPIC_INVOICE_OVERDUE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceOverdue {
    /// Invoice id.
    pub invoice_id: String,
    /// Original due date.
    pub due_at: DateTime<Utc>,
    /// Audit actor (typically `inv.overdue.ticker`).
    pub actor: String,
}

/// Payload for [`TOPIC_INVOICE_VOIDED`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvoiceVoided {
    /// Invoice id.
    pub invoice_id: String,
    /// Optional reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Audit actor.
    pub actor: String,
    /// Channel.
    pub channel: String,
}

// =============================================================================
// Credit-note domain payloads
// =============================================================================

/// Payload for [`TOPIC_CREDITNOTE_DRAFTED`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditNoteDrafted {
    /// Credit-note id.
    pub credit_note_id: String,
    /// Invoice the note credits.
    pub invoice_id: String,
    /// Amount (Decimal as string).
    pub amount: String,
    /// Currency.
    pub currency: String,
    /// Inbound refund event id, if auto-created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refund_ref: Option<String>,
    /// Audit actor.
    pub actor: String,
    /// Channel.
    pub channel: String,
}

/// Payload for [`TOPIC_CREDITNOTE_ISSUED`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreditNoteIssued {
    /// Credit-note id.
    pub credit_note_id: String,
    /// Assigned number (`CN-YYYY-NNNN`).
    pub number: Option<String>,
    /// Invoice.
    pub invoice_id: String,
    /// Amount.
    pub amount: String,
    /// Currency.
    pub currency: String,
    /// Audit actor.
    pub actor: String,
    /// Channel.
    pub channel: String,
}

// =============================================================================
// Reminder payloads
// =============================================================================

/// Payload for [`TOPIC_REMINDER_SCHEDULED`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReminderScheduled {
    /// Reminder id.
    pub reminder_id: String,
    /// Invoice the reminder targets.
    pub invoice_id: String,
    /// When the reminder is scheduled to fire.
    pub due_at: DateTime<Utc>,
    /// Reminder channel (`email`, `webhook`, …).
    pub channel: String,
    /// Audit actor.
    pub actor: String,
}

/// Payload for [`TOPIC_REMINDER_SENT`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReminderSent {
    /// Reminder id.
    pub reminder_id: String,
    /// Invoice.
    pub invoice_id: String,
    /// Outbound channel actually used.
    pub channel: String,
    /// When the send completed.
    pub sent_at: DateTime<Utc>,
}

/// Payload for [`TOPIC_REMINDER_CANCELLED`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReminderCancelled {
    /// Reminder id.
    pub reminder_id: String,
    /// Invoice.
    pub invoice_id: String,
    /// Optional reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Audit actor.
    pub actor: String,
}

// =============================================================================
// Schedule payloads
// =============================================================================

/// Payload for [`TOPIC_SCHEDULE_CREATED`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleCreated {
    /// Schedule id.
    pub schedule_id: String,
    /// Bill-to customer id.
    pub customer_id: String,
    /// Cadence (`monthly`, `weekly`, …).
    pub cadence: String,
    /// Currency.
    pub currency: String,
    /// Auto-issue flag.
    pub auto_issue: bool,
    /// First-run date.
    pub start_date: NaiveDate,
    /// Audit actor.
    pub actor: String,
    /// Channel.
    pub channel: String,
}

/// Payload for [`TOPIC_SCHEDULE_PAUSED`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulePaused {
    /// Schedule id.
    pub schedule_id: String,
    /// New state (mirrors enum, lower-cased).
    pub state: String,
    /// Audit actor.
    pub actor: String,
    /// Channel.
    pub channel: String,
}

/// Payload for [`TOPIC_SCHEDULE_CANCELLED`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleCancelled {
    /// Schedule id.
    pub schedule_id: String,
    /// New state.
    pub state: String,
    /// Optional reason (`end_date_reached`, operator note, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_constants_match_design_4_2() {
        assert_eq!(TOPIC_INVOICE_DRAFTED, "inv.billing.invoice.drafted");
        assert_eq!(TOPIC_INVOICE_ISSUED, "inv.billing.invoice.issued");
        assert_eq!(TOPIC_INVOICE_SENT, "inv.billing.invoice.sent");
        assert_eq!(TOPIC_INVOICE_VIEWED, "inv.billing.invoice.viewed");
        assert_eq!(TOPIC_INVOICE_PAID, "inv.billing.invoice.paid");
        assert_eq!(
            TOPIC_INVOICE_PARTIALLY_PAID,
            "inv.billing.invoice.partially_paid"
        );
        assert_eq!(TOPIC_INVOICE_OVERDUE, "inv.billing.invoice.overdue");
        assert_eq!(TOPIC_INVOICE_VOIDED, "inv.billing.invoice.voided");
        assert_eq!(TOPIC_CREDITNOTE_DRAFTED, "inv.billing.creditnote.drafted");
        assert_eq!(TOPIC_CREDITNOTE_ISSUED, "inv.billing.creditnote.issued");
        assert_eq!(TOPIC_REMINDER_SCHEDULED, "inv.billing.reminder.scheduled");
        assert_eq!(TOPIC_REMINDER_SENT, "inv.billing.reminder.sent");
        assert_eq!(TOPIC_REMINDER_CANCELLED, "inv.billing.reminder.cancelled");
        assert_eq!(TOPIC_SCHEDULE_CREATED, "inv.billing.schedule.created");
        assert_eq!(TOPIC_SCHEDULE_PAUSED, "inv.billing.schedule.paused");
        assert_eq!(TOPIC_SCHEDULE_CANCELLED, "inv.billing.schedule.cancelled");
    }

    #[test]
    fn invoice_drafted_round_trips() {
        let p = InvoiceDrafted {
            invoice_id: "inv_x".into(),
            customer_id: "cust_x".into(),
            currency: "CAD".into(),
            subtotal: "100.00".into(),
            total: "100.00".into(),
            actor: "jad".into(),
            channel: "cli".into(),
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: InvoiceDrafted = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }
}
