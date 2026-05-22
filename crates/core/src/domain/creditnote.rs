//! Credit notes — separate entity, simpler FSM than invoices.
//!
//! A credit note is always linked to an invoice. The lifecycle is
//! `Draft → Issued` (no further states at v1). The amount is in the
//! same currency as the original invoice.
//!
//! Refunds (via `fin.billing.payment.refunded`) auto-create a draft
//! credit note; the operator or agent issues it.

use super::ids::{CreditNoteId, HistoryId, InvoiceId};
use super::invoice::HistoryChannel;
use super::money::Currency;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Lifecycle state of a credit note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum CreditNoteState {
    /// Mutable.
    Draft,
    /// Frozen, numbered — terminal.
    Issued,
}

/// A credit note against an invoice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreditNote {
    /// Stable identifier.
    pub id: CreditNoteId,
    /// Sequence-driven human number, assigned on `Issued`. Format:
    /// `CN-YYYY-NNNN`. `None` while in `Draft`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    /// Invoice this note credits.
    pub invoice_id: InvoiceId,
    /// Lifecycle state.
    pub state: CreditNoteState,
    /// Amount being credited (positive Decimal in the invoice currency).
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub amount: Decimal,
    /// Same currency as the linked invoice.
    pub currency: Currency,
    /// Free-form reason note (e.g., "duplicate billing").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// If the credit note was auto-created from a refund event, the
    /// inbound bus event id is recorded here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refund_ref: Option<String>,
    /// When the credit note moved `draft → issued`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_at: Option<DateTime<Utc>>,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// DB insert time.
    pub created_at: DateTime<Utc>,
}

/// A row in the credit-note state-history table (audit + outbox).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreditNoteStateHistory {
    /// Stable identifier.
    pub id: HistoryId,
    /// Owning credit note.
    pub credit_note_id: CreditNoteId,
    /// Previous state (`None` for the initial `drafted` row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_state: Option<CreditNoteState>,
    /// New state.
    pub to_state: CreditNoteState,
    /// Transition trigger.
    pub event: String,
    /// Who triggered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Channel.
    pub channel: HistoryChannel,
    /// Inbound bus event_id, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bus_event_id: Option<String>,
    /// When the transition happened.
    pub occurred_at: DateTime<Utc>,
    /// Outbox marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<DateTime<Utc>>,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::FromStr;

    #[test]
    fn credit_note_state_serde() {
        assert_eq!(serde_json::to_string(&CreditNoteState::Draft).unwrap(), r#""draft""#);
        assert_eq!(serde_json::to_string(&CreditNoteState::Issued).unwrap(), r#""issued""#);
    }

    #[test]
    fn credit_note_round_trips() {
        let n = CreditNote {
            id: CreditNoteId::new(),
            number: Some("CN-2026-0001".into()),
            invoice_id: InvoiceId::new(),
            state: CreditNoteState::Issued,
            amount: Decimal::from_str("250.00").unwrap(),
            currency: Currency::USD,
            reason: Some("duplicate billing".into()),
            refund_ref: None,
            issued_at: Some(Utc::now()),
            metadata: BTreeMap::new(),
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&n).unwrap();
        let back: CreditNote = serde_json::from_str(&json).unwrap();
        assert_eq!(n, back);
    }
}
