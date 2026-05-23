//! Invoice + invoice line types.
//!
//! State transitions live in `crate::domain::state` (T-0005). This module
//! is pure data: no FSM gates, no command logic, no persistence.

use super::ids::{CustomerId, HistoryId, InvoiceId, LineId, ScheduleId};
use super::jurisdiction::Jurisdiction;
use super::money::Currency;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Lifecycle state of an invoice.
///
/// See design spec §3.4 for the full transition diagram; this enum only
/// names the states. `Overdue` is intentionally NOT here — it's a derived
/// flag + bus event, not an FSM state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum InvoiceState {
    /// Mutable: lines, customer, dates may change.
    Draft,
    /// Frozen, numbered, tax-snapshotted; payment may land at any time.
    Issued,
    /// Delivered via at least one channel.
    Sent,
    /// Customer fetched the rendered document.
    Viewed,
    /// Some but not all of the amount has been received.
    PartiallyPaid,
    /// Fully paid — terminal.
    Paid,
    /// Voided before any payment — terminal.
    Voided,
}

/// Tax category applied to a single line.
///
/// Per design §6.1, the default is `Standard`. `Reduced` looks up the
/// rate's reduced row (falls back to standard with a warning if absent).
/// `ZeroRated` is 0 % with the rate id recorded (audit shows "explicitly
/// zero-rated"). `Exempt` is 0 % with different audit semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum TaxCategory {
    /// Default rate for the jurisdiction.
    #[default]
    Standard,
    /// Reduced rate (jurisdiction-specific eligibility, e.g. DZ TVA 9 %).
    Reduced,
    /// 0 %, but the rate id is still recorded for audit.
    ZeroRated,
    /// 0 %, recorded as exempt (different audit semantics from zero-rated).
    Exempt,
}

/// A single line on an invoice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InvoiceLine {
    /// Stable identifier.
    pub id: LineId,
    /// Owning invoice.
    pub invoice_id: InvoiceId,
    /// Position in the invoice's line list (0-based, dense).
    pub position: u32,
    /// Free-form description.
    pub description: String,
    /// Quantity (Decimal — supports 0.5 hours, 1.25 kg, etc.).
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub quantity: Decimal,
    /// Per-unit price in invoice currency.
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub unit_price: Decimal,
    /// Tax rates applied (typeid-ish ids from `tax_rates` table — possibly
    /// multiple, e.g., GST + QST on a QC sale).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tax_rate_ids: Vec<String>,
    /// Category override; defaults to `Standard`.
    #[serde(default)]
    pub tax_category: TaxCategory,
    /// Tax amount on this line (sum across applied rates).
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub tax_amount: Decimal,
    /// `(quantity * unit_price) + tax_amount`.
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub line_total: Decimal,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

/// An invoice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Invoice {
    /// Stable identifier.
    pub id: InvoiceId,
    /// Sequence-driven human number, assigned when transitioning to `Issued`.
    /// Format: `INV-YYYY-NNNN`. `None` while in `Draft`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    /// Bill-to customer.
    pub customer_id: CustomerId,
    /// Seller jurisdiction (for tax resolution).
    pub seller_jurisdiction: Jurisdiction,
    /// Invoice currency (every line + total is in this currency).
    pub currency: Currency,
    /// Lifecycle state.
    pub state: InvoiceState,

    /// When the invoice moved `draft → issued`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_at: Option<DateTime<Utc>>,
    /// Payment due date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due_at: Option<DateTime<Utc>>,
    /// Most-recent send timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_at: Option<DateTime<Utc>>,
    /// First view timestamp (signed-link channel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewed_at: Option<DateTime<Utc>>,
    /// When the invoice moved into a paid state (partial counts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paid_at: Option<DateTime<Utc>>,
    /// When the invoice was voided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voided_at: Option<DateTime<Utc>>,

    /// Sum of line subtotals (before tax).
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub subtotal: Decimal,
    /// Sum of line tax amounts.
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub tax_total: Decimal,
    /// `subtotal + tax_total`.
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub total: Decimal,
    /// Sum of payments received against this invoice so far.
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub amount_paid: Decimal,

    /// If non-null, the recurring schedule that produced this invoice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_id: Option<ScheduleId>,
    /// Per-invoice template override; falls back to the global config path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_path: Option<String>,
    /// Blob URI for the rendered PDF (set after a successful render).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pdf_blob_ref: Option<String>,
    /// Caller-supplied dedupe key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// Set when the tax engine couldn't determine US nexus and the operator
    /// should review before sending.
    #[serde(default)]
    pub nexus_review: bool,

    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// DB insert time.
    pub created_at: DateTime<Utc>,
    /// Last modified.
    pub updated_at: DateTime<Utc>,
}

/// A row in the invoice state-history table.
///
/// Doubles as the transactional outbox: when `published_at` is `None`,
/// the bus relay still needs to publish the corresponding event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InvoiceStateHistory {
    /// Stable identifier (one row per transition).
    pub id: HistoryId,
    /// Owning invoice.
    pub invoice_id: InvoiceId,
    /// Previous state (`None` for the initial `drafted` row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_state: Option<InvoiceState>,
    /// State the invoice moved into.
    pub to_state: InvoiceState,
    /// Transition trigger (`issue`, `send`, `view`, `pay`, `partial_pay`,
    /// `void`, etc. — see commands layer).
    pub event: String,
    /// Who triggered (user name, agent id, `fin.bus.consumer`, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Channel the transition came in on.
    pub channel: HistoryChannel,
    /// Inbound bus event_id, if the transition was triggered by one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bus_event_id: Option<String>,
    /// Free-form reason note (void reason, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// When the transition happened.
    pub occurred_at: DateTime<Utc>,
    /// Outbox marker: `None` = pending publish; `Some(t)` = published at `t`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<DateTime<Utc>>,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

/// Channel an FSM transition (or any command) arrived through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum HistoryChannel {
    /// Triggered by the CLI adapter.
    Cli,
    /// Triggered by the HTTP API adapter.
    Api,
    /// Triggered by the WebSocket adapter.
    Ws,
    /// Triggered by the MCP server adapter.
    Mcp,
    /// Triggered by an inbound bus event consumer.
    Bus,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::FromStr;

    #[test]
    fn invoice_state_serde() {
        let s = serde_json::to_string(&InvoiceState::PartiallyPaid).unwrap();
        assert_eq!(s, r#""partially_paid""#);
        let back: InvoiceState = serde_json::from_str(&s).unwrap();
        assert_eq!(back, InvoiceState::PartiallyPaid);
    }

    #[test]
    fn tax_category_default_is_standard() {
        assert_eq!(TaxCategory::default(), TaxCategory::Standard);
        assert_eq!(
            serde_json::to_string(&TaxCategory::ZeroRated).unwrap(),
            r#""zero_rated""#
        );
    }

    #[test]
    fn invoice_line_round_trips() {
        let inv_id = InvoiceId::new();
        let line = InvoiceLine {
            id: LineId::new(),
            invoice_id: inv_id.clone(),
            position: 0,
            description: "Consulting hours".into(),
            quantity: Decimal::from_str("10").unwrap(),
            unit_price: Decimal::from_str("125.00").unwrap(),
            tax_rate_ids: vec!["ca-qc-gst".into(), "ca-qc-qst".into()],
            tax_category: TaxCategory::Standard,
            tax_amount: Decimal::from_str("187.19").unwrap(),
            line_total: Decimal::from_str("1437.19").unwrap(),
            metadata: BTreeMap::new(),
        };
        let json = serde_json::to_string(&line).unwrap();
        let back: InvoiceLine = serde_json::from_str(&json).unwrap();
        assert_eq!(line, back);
    }

    #[test]
    fn draft_invoice_round_trips() {
        let inv = Invoice {
            id: InvoiceId::new(),
            number: None,
            customer_id: CustomerId::new(),
            seller_jurisdiction: Jurisdiction::QuebecCa,
            currency: Currency::CAD,
            state: InvoiceState::Draft,
            issued_at: None,
            due_at: None,
            sent_at: None,
            viewed_at: None,
            paid_at: None,
            voided_at: None,
            subtotal: Decimal::ZERO,
            tax_total: Decimal::ZERO,
            total: Decimal::ZERO,
            amount_paid: Decimal::ZERO,
            schedule_id: None,
            template_path: None,
            pdf_blob_ref: None,
            idempotency_key: None,
            nexus_review: false,
            metadata: BTreeMap::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = serde_json::to_string(&inv).unwrap();
        let back: Invoice = serde_json::from_str(&json).unwrap();
        assert_eq!(inv, back);
    }

    #[test]
    fn history_channel_serde() {
        assert_eq!(
            serde_json::to_string(&HistoryChannel::Cli).unwrap(),
            r#""cli""#
        );
        assert_eq!(
            serde_json::to_string(&HistoryChannel::Bus).unwrap(),
            r#""bus""#
        );
    }

    #[test]
    fn history_row_round_trips() {
        let h = InvoiceStateHistory {
            id: HistoryId::new(),
            invoice_id: InvoiceId::new(),
            from_state: Some(InvoiceState::Draft),
            to_state: InvoiceState::Issued,
            event: "issue".into(),
            actor: Some("jad".into()),
            channel: HistoryChannel::Cli,
            bus_event_id: None,
            reason: None,
            occurred_at: Utc::now(),
            published_at: None,
            metadata: BTreeMap::new(),
        };
        let json = serde_json::to_string(&h).unwrap();
        let back: InvoiceStateHistory = serde_json::from_str(&json).unwrap();
        assert_eq!(h, back);
    }
}
