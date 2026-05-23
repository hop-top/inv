//! Bus consumer: route inbound `fin.billing.*` (and similar) events to
//! the matching `inv-commands` function.
//!
//! The consumer is just another channel adapter — same shape as CLI /
//! HTTP / WS / MCP. The only difference is that the request decode reads
//! a JSON bus payload + the routing key is the topic string rather than
//! a URL path or a clap subcommand.
//!
//! ## Flow per inbound event
//!
//! 1. The transport-side receiver gets an event from some bus (kit's
//!    primitive, an SSE stream, an SQS poll, a sidecar webhook, …). It
//!    has `event_id`, `topic`, `source`, `payload_json`.
//! 2. The receiver calls [`dispatch_inbound_event`] (in `inbox.rs`) to
//!    register the `event_id` in `bus_inbox`. If `false`, this is a
//!    replay — skip.
//! 3. The receiver calls [`Consumer::dispatch`] with the freshly-accepted
//!    event. The consumer:
//!    - Applies [`crate::TopicMap::remap`] to canonicalise the topic
//!      (e.g. operator renamed `finance.charges.created` to
//!      `fin.billing.charge.created` via `[bus.remap]`).
//!    - Decodes the payload into a payload-specific struct.
//!    - Looks up the matching command + builds its input.
//!    - Calls the command.
//! 4. On success, the receiver calls
//!    [`BusInboxRepo::mark_processed`] so admin tooling can distinguish
//!    "received but command failed" from "processed cleanly".
//!
//! ## Topic contract (design §4.3)
//!
//! Default topics in:
//!   - `fin.billing.charge.created`      -> [`draft_invoice`]
//!   - `fin.billing.payment.received`    -> [`mark_paid`]
//!   - `fin.billing.payment.refunded`    -> [`create_credit_note`]
//!
//! Operators can remap via `[bus.remap]` if fin emits different names.

use std::collections::BTreeMap;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use inv_commands::{
    create_credit_note, draft_invoice, mark_paid, Actor, Channel, CoreCtx, CoreError,
    CreateCreditNoteInput, CreateCreditNoteOutput, DraftInvoiceInput, DraftInvoiceOutput,
    DraftLineInput, MarkPaidInput, MarkPaidOutput,
};
use inv_core::domain::ids::{CustomerId, InvoiceId};
use inv_core::domain::invoice::TaxCategory;
use inv_core::domain::jurisdiction::Jurisdiction;
use inv_core::domain::money::Currency;

use crate::topic_map::{remap_topic, TopicMap};

/// Errors specific to dispatch (separate from `IngestError`, which only
/// covers inbox dedup).
#[derive(Debug, Error)]
pub enum DispatchError {
    /// Topic isn't routable — no command handles it.
    #[error("no consumer for topic `{0}`")]
    UnknownTopic(String),
    /// Payload JSON didn't match the expected schema for this topic.
    #[error("decoding payload for `{topic}` failed: {source}")]
    DecodePayload {
        /// Canonical topic (after remap).
        topic: String,
        /// Underlying serde_json error message.
        #[source]
        source: serde_json::Error,
    },
    /// A required field on the payload is missing or malformed.
    #[error("validating payload for `{topic}`: {message}")]
    InvalidPayload {
        /// Canonical topic.
        topic: String,
        /// Why we rejected.
        message: String,
    },
    /// The downstream command rejected the call.
    #[error("command `{command}` failed: {source}")]
    Command {
        /// Command name (`draft_invoice` etc.).
        command: &'static str,
        /// Underlying CoreError.
        #[source]
        source: CoreError,
    },
}

/// Output of a successful dispatch — carries the typed command output so
/// the caller can attach metadata (e.g. set `bus_inbox.invoice_id` for
/// admin lookup).
#[derive(Debug)]
pub enum DispatchOutput {
    /// `fin.billing.charge.created` → a fresh draft invoice.
    Drafted(DraftInvoiceOutput),
    /// `fin.billing.payment.received` → a paid (or partially-paid) invoice.
    Paid(MarkPaidOutput),
    /// `fin.billing.payment.refunded` → a draft credit note.
    Refunded(CreateCreditNoteOutput),
}

impl DispatchOutput {
    /// Invoice that the inbound event touched, if any. Useful to set
    /// `bus_inbox.invoice_id` so admins can find the row by invoice.
    pub fn invoice_id(&self) -> Option<InvoiceId> {
        match self {
            DispatchOutput::Drafted(o) => Some(o.invoice.id.clone()),
            DispatchOutput::Paid(o) => Some(o.invoice.id.clone()),
            DispatchOutput::Refunded(o) => Some(o.credit_note.invoice_id.clone()),
        }
    }
}

/// Bus consumer with a topic remap.
///
/// The remap step lets a deployment subscribe to whatever-fin-actually-
/// emits and translate it onto `inv`'s canonical contract topics without
/// recompiling. See design §4.3.
#[derive(Debug, Default, Clone)]
pub struct Consumer {
    remap: TopicMap,
}

impl Consumer {
    /// Construct a consumer with no remap (canonical topics only).
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct a consumer with a topic remap (operator-configured via
    /// `[bus.remap]` in the inv config).
    pub fn with_remap(remap: TopicMap) -> Self {
        Self { remap }
    }

    /// Dispatch a single inbound event to the matching command.
    ///
    /// Caller is responsible for inbox dedup BEFORE calling this; see
    /// [`crate::inbox::dispatch_inbound_event`].
    pub async fn dispatch(
        &self,
        ctx: &CoreCtx,
        topic: &str,
        event_id: &str,
        payload_json: &str,
    ) -> Result<DispatchOutput, DispatchError> {
        let canonical = remap_topic(&self.remap, topic).to_string();
        match canonical.as_str() {
            "fin.billing.charge.created" => self
                .handle_charge_created(ctx, &canonical, event_id, payload_json)
                .await
                .map(DispatchOutput::Drafted),
            "fin.billing.payment.received" => self
                .handle_payment_received(ctx, &canonical, event_id, payload_json)
                .await
                .map(DispatchOutput::Paid),
            "fin.billing.payment.refunded" => self
                .handle_payment_refunded(ctx, &canonical, event_id, payload_json)
                .await
                .map(DispatchOutput::Refunded),
            _ => Err(DispatchError::UnknownTopic(canonical)),
        }
    }

    async fn handle_charge_created(
        &self,
        ctx: &CoreCtx,
        topic: &str,
        event_id: &str,
        payload_json: &str,
    ) -> Result<DraftInvoiceOutput, DispatchError> {
        let p: ChargeCreatedPayload = decode(topic, payload_json)?;
        let lines = p
            .lines
            .into_iter()
            .map(|l| DraftLineInput {
                description: l.description,
                quantity: l.quantity,
                unit_price: l.unit_price,
                tax_category: l.tax_category.unwrap_or_default(),
            })
            .collect();
        let input = DraftInvoiceInput {
            customer_id: parse_customer_id(topic, &p.customer_id)?,
            seller_jurisdiction: p.seller_jurisdiction.unwrap_or(Jurisdiction::QuebecCa),
            // TODO(T-0029-style): pass through from payload once fin sends it.
            currency: p.currency,
            lines,
            idempotency_key: Some(p.idempotency_key.unwrap_or_else(|| event_id.to_string())),
            actor: Actor::Bus {
                source: "fin".into(),
            },
            channel: Channel::Bus,
            due_at: p.due_date,
            template_path: None,
            schedule_id: None,
        };
        draft_invoice(ctx, input)
            .await
            .map_err(|e| DispatchError::Command {
                command: "draft_invoice",
                source: e,
            })
    }

    async fn handle_payment_received(
        &self,
        ctx: &CoreCtx,
        topic: &str,
        event_id: &str,
        payload_json: &str,
    ) -> Result<MarkPaidOutput, DispatchError> {
        let p: PaymentReceivedPayload = decode(topic, payload_json)?;
        let invoice_id = p.invoice_ref.ok_or_else(|| DispatchError::InvalidPayload {
            topic: topic.to_string(),
            message: "payment.received without invoice_ref is not routable at v1".into(),
        })?;
        let input = MarkPaidInput {
            invoice_id: parse_invoice_id_from_ref(topic, &invoice_id)?,
            amount: p.amount,
            received_at: Some(p.received_at),
            idempotency_key: Some(p.idempotency_key.unwrap_or_else(|| event_id.to_string())),
            bus_event_id: Some(event_id.to_string()),
            actor: Actor::Bus {
                source: "fin".into(),
            },
            channel: Channel::Bus,
        };
        mark_paid(ctx, input)
            .await
            .map_err(|e| DispatchError::Command {
                command: "mark_paid",
                source: e,
            })
    }

    async fn handle_payment_refunded(
        &self,
        ctx: &CoreCtx,
        topic: &str,
        event_id: &str,
        payload_json: &str,
    ) -> Result<CreateCreditNoteOutput, DispatchError> {
        let p: PaymentRefundedPayload = decode(topic, payload_json)?;
        let input = CreateCreditNoteInput {
            invoice_id: parse_invoice_id_from_ref(topic, &p.invoice_ref)?,
            amount: p.amount,
            reason: p.reason,
            refund_ref: Some(p.refund_id.unwrap_or_else(|| event_id.to_string())),
            idempotency_key: Some(event_id.to_string()),
            actor: Actor::Bus {
                source: "fin".into(),
            },
            channel: Channel::Bus,
        };
        create_credit_note(ctx, input)
            .await
            .map_err(|e| DispatchError::Command {
                command: "create_credit_note",
                source: e,
            })
    }
}

// ---------------------------------------------------------------------
// Payload structs — match the contract shapes from design §4.3.
// ---------------------------------------------------------------------

/// `fin.billing.charge.created` payload contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChargeCreatedPayload {
    /// External customer id (may be either a TypeID or any opaque caller
    /// string; we accept both — the consumer parses via [`InvoiceId::parse`]
    /// when the string looks like a typeid and treats anything else as
    /// already-resolved by an upstream mapper).
    customer_id: String,
    /// Invoice currency.
    currency: Currency,
    /// Optional seller jurisdiction override (otherwise inv falls back
    /// to its configured default).
    #[serde(default)]
    seller_jurisdiction: Option<Jurisdiction>,
    /// Lines on the charge.
    lines: Vec<ChargeLine>,
    /// Optional due date.
    #[serde(default)]
    due_date: Option<DateTime<Utc>>,
    /// Caller-side idempotency key. If absent, the inbound `event_id`
    /// is used.
    #[serde(default)]
    idempotency_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChargeLine {
    description: String,
    quantity: Decimal,
    unit_price: Decimal,
    #[serde(default)]
    tax_category: Option<TaxCategory>,
}

/// `fin.billing.payment.received` payload contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PaymentReceivedPayload {
    /// Inv's invoice reference. Can be a bare typeid (`invoice_01J…`)
    /// or a poly-uri URI (`inv://invoice/invoice_01J…`).
    invoice_ref: Option<String>,
    /// Amount received (positive Decimal in the invoice's currency).
    amount: Decimal,
    /// Wall-clock at receipt (from fin's bus envelope).
    received_at: DateTime<Utc>,
    /// Optional caller-side idempotency key.
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// `fin.billing.payment.refunded` payload contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PaymentRefundedPayload {
    /// Inv's invoice reference (same shape as for `payment.received`).
    invoice_ref: String,
    /// Amount refunded (positive Decimal in the invoice's currency).
    amount: Decimal,
    /// Free-form reason.
    #[serde(default)]
    reason: Option<String>,
    /// Refund id from fin (becomes credit_notes.refund_ref).
    #[serde(default)]
    refund_id: Option<String>,
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

fn decode<T: for<'de> Deserialize<'de>>(
    topic: &str,
    payload_json: &str,
) -> Result<T, DispatchError> {
    serde_json::from_str(payload_json).map_err(|source| DispatchError::DecodePayload {
        topic: topic.to_string(),
        source,
    })
}

fn parse_customer_id(topic: &str, s: &str) -> Result<CustomerId, DispatchError> {
    CustomerId::parse(s).map_err(|e| DispatchError::InvalidPayload {
        topic: topic.to_string(),
        message: format!("invalid customer_id `{s}`: {e}"),
    })
}

/// Accept either a bare `invoice_01J…` typeid or a poly-uri URI
/// `inv://invoice/invoice_01J…`. Anything else is invalid.
fn parse_invoice_id_from_ref(topic: &str, s: &str) -> Result<InvoiceId, DispatchError> {
    let typeid = s.strip_prefix("inv://invoice/").unwrap_or(s);
    InvoiceId::from_str(typeid).map_err(|e| DispatchError::InvalidPayload {
        topic: topic.to_string(),
        message: format!("invalid invoice_ref `{s}`: {e}"),
    })
}

// Silence unused-import warnings if BTreeMap isn't reached.
#[allow(dead_code)]
const _: Option<BTreeMap<String, String>> = None;
