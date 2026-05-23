//! Op dispatcher — maps a string op name + JSON payload onto the
//! matching `hop-top-inv-commands` function.
//!
//! The wire-shape inputs use serde-derived structs that mirror the
//! command-layer `*Input` types one-for-one. We deliberately keep this
//! a separate set of structs (not the `hop_top_inv_commands::*Input` types
//! directly) so the wire shape can evolve independently — e.g. a
//! command Input might gain an internal-only field that we don't want
//! to expose, or vice versa.

use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use hop_top_inv_commands::{
    create_credit_note, draft_invoice, issue_credit_note, issue_invoice, mark_overdue_ticker,
    mark_paid, reminder_cancel, reminder_schedule, reminders_tick, schedule_cancel,
    schedule_create, schedule_pause, schedules_tick, send_invoice, void_invoice, Actor, Channel,
    CoreCtx, CoreError, CreateCreditNoteInput, DraftInvoiceInput, DraftLineInput,
    IssueCreditNoteInput, IssueInvoiceInput, MarkPaidInput, ReminderCancelInput,
    ReminderScheduleInput, ScheduleCreateInput, ScheduleLineInput, ScheduleStateChangeInput,
    SendInvoiceInput, VoidInvoiceInput,
};
use hop_top_inv_core::domain::ids::{CreditNoteId, CustomerId, InvoiceId, ReminderId, ScheduleId};
use hop_top_inv_core::domain::invoice::TaxCategory;
use hop_top_inv_core::domain::jurisdiction::Jurisdiction;
use hop_top_inv_core::domain::money::Currency;
use hop_top_inv_core::domain::reminder::ReminderChannel;
use hop_top_inv_core::domain::schedule::Cadence;

use crate::frames::ErrorBody;

/// Outcome of an op dispatch.
pub type DispatchResult = Result<Value, OpError>;

/// Anything that can go wrong dispatching an op.
#[derive(Debug)]
pub struct OpError {
    /// Short tag (`"unknown_op"`, `"validation"`, `"not_found"`, `"internal"`, …).
    pub code: &'static str,
    /// Human-readable explanation.
    pub message: String,
}

impl OpError {
    /// Render as the on-the-wire `ErrorBody`.
    pub fn into_error_body(self) -> ErrorBody {
        ErrorBody {
            code: self.code.to_string(),
            message: self.message,
        }
    }

    fn validation(msg: impl Into<String>) -> Self {
        Self {
            code: "validation",
            message: msg.into(),
        }
    }

    fn from_core(e: CoreError) -> Self {
        // Map CoreError variants onto error-code tags the wire protocol
        // exposes. Most map to "internal" by default; only the variants
        // we want clients to react to programmatically get their own tag.
        let code = match &e {
            CoreError::Validation(_) => "validation",
            CoreError::Idempotency(_) => "idempotency",
            CoreError::NotFound(_) => "not_found",
            CoreError::NotImplemented(_) => "not_implemented",
            CoreError::FsmTransition(_) => "fsm_transition",
            CoreError::Tax(_) => "tax",
            CoreError::Render(_) | CoreError::Pdf(_) => "render",
            CoreError::Repo(_) => "repo",
        };
        Self {
            code,
            message: e.to_string(),
        }
    }
}

/// Dispatch a single request frame.
///
/// `actor_name` identifies the WebSocket session principal — at v1 this
/// is whatever the upgrade route resolved (often anonymous; T-0019 will
/// wire it onto an auth layer).
pub async fn dispatch(
    ctx: &Arc<CoreCtx>,
    op: &str,
    payload: Value,
    actor_name: &str,
) -> DispatchResult {
    let actor = Actor::Ws {
        name: actor_name.to_string(),
    };
    let channel = Channel::Ws;

    match op {
        // -----------------------------------------------------------------
        // Invoice
        // -----------------------------------------------------------------
        "invoice.draft" => {
            let wire: DraftInvoiceWire = decode(payload)?;
            let input = wire.into_input(actor, channel)?;
            let out = draft_invoice(ctx, input)
                .await
                .map_err(OpError::from_core)?;
            Ok(json!({
                "invoice": out.invoice,
                "lines": out.lines,
                "idempotency_replay": out.idempotency_replay,
            }))
        }
        "invoice.issue" => {
            let wire: InvoiceIdWire = decode(payload)?;
            let input = IssueInvoiceInput {
                invoice_id: wire.invoice_id()?,
                idempotency_key: wire.idempotency_key,
                actor,
                channel,
            };
            let out = issue_invoice(ctx, input)
                .await
                .map_err(OpError::from_core)?;
            Ok(json!({
                "invoice": out.invoice,
                "lines": out.lines,
                "html": out.html,
            }))
        }
        "invoice.send" => {
            let wire: SendInvoiceWire = decode(payload)?;
            let invoice_id = parse_invoice_id(&wire.invoice_id)?;
            let input = SendInvoiceInput {
                invoice_id,
                destination_uri: wire.destination_uri,
                idempotency_key: wire.idempotency_key,
                actor,
                channel,
                sink: None,
            };
            let out = send_invoice(ctx, input).await.map_err(OpError::from_core)?;
            Ok(json!({
                "invoice": out.invoice,
                "delivered_to": out.delivered_to,
            }))
        }
        "invoice.pay" => {
            let wire: MarkPaidWire = decode(payload)?;
            let input = MarkPaidInput {
                invoice_id: parse_invoice_id(&wire.invoice_id)?,
                amount: wire.amount,
                received_at: wire.received_at,
                idempotency_key: wire.idempotency_key,
                bus_event_id: wire.bus_event_id,
                actor,
                channel,
            };
            let out = mark_paid(ctx, input).await.map_err(OpError::from_core)?;
            Ok(json!({
                "invoice": out.invoice,
                "fully_paid": out.fully_paid,
            }))
        }
        "invoice.void" => {
            let wire: VoidInvoiceWire = decode(payload)?;
            let input = VoidInvoiceInput {
                invoice_id: parse_invoice_id(&wire.invoice_id)?,
                reason: wire.reason,
                idempotency_key: wire.idempotency_key,
                actor,
                channel,
            };
            let out = void_invoice(ctx, input).await.map_err(OpError::from_core)?;
            Ok(json!({ "invoice": out.invoice }))
        }

        // -----------------------------------------------------------------
        // Credit note
        // -----------------------------------------------------------------
        "creditnote.draft" => {
            let wire: CreateCreditNoteWire = decode(payload)?;
            let input = CreateCreditNoteInput {
                invoice_id: parse_invoice_id(&wire.invoice_id)?,
                amount: wire.amount,
                reason: wire.reason,
                refund_ref: wire.refund_ref,
                idempotency_key: wire.idempotency_key,
                actor,
                channel,
            };
            let out = create_credit_note(ctx, input)
                .await
                .map_err(OpError::from_core)?;
            Ok(json!({ "credit_note": out.credit_note }))
        }
        "creditnote.issue" => {
            let wire: CreditNoteIdWire = decode(payload)?;
            let input = IssueCreditNoteInput {
                credit_note_id: parse_credit_note_id(&wire.credit_note_id)?,
                idempotency_key: wire.idempotency_key,
                actor,
                channel,
            };
            let out = issue_credit_note(ctx, input)
                .await
                .map_err(OpError::from_core)?;
            Ok(json!({ "credit_note": out.credit_note }))
        }

        // -----------------------------------------------------------------
        // Schedule
        // -----------------------------------------------------------------
        "schedule.create" => {
            let wire: ScheduleCreateWire = decode(payload)?;
            let input = wire.into_input(actor, channel)?;
            let out = schedule_create(ctx, input)
                .await
                .map_err(OpError::from_core)?;
            Ok(json!({ "schedule": out.schedule }))
        }
        "schedule.pause" => {
            let wire: ScheduleIdWire = decode(payload)?;
            let input = ScheduleStateChangeInput {
                schedule_id: parse_schedule_id(&wire.schedule_id)?,
                actor,
                channel,
            };
            let out = schedule_pause(ctx, input)
                .await
                .map_err(OpError::from_core)?;
            Ok(json!({ "schedule": out.schedule }))
        }
        "schedule.cancel" => {
            let wire: ScheduleIdWire = decode(payload)?;
            let input = ScheduleStateChangeInput {
                schedule_id: parse_schedule_id(&wire.schedule_id)?,
                actor,
                channel,
            };
            let out = schedule_cancel(ctx, input)
                .await
                .map_err(OpError::from_core)?;
            Ok(json!({ "schedule": out.schedule }))
        }

        // -----------------------------------------------------------------
        // Reminder
        // -----------------------------------------------------------------
        "reminder.schedule" => {
            let wire: ReminderScheduleWire = decode(payload)?;
            let input = ReminderScheduleInput {
                invoice_id: parse_invoice_id(&wire.invoice_id)?,
                scheduled_at: wire.scheduled_at,
                channel_scheme: wire.channel_scheme,
                actor,
                channel,
            };
            let out = reminder_schedule(ctx, input)
                .await
                .map_err(OpError::from_core)?;
            Ok(json!({ "reminder": out.reminder }))
        }
        "reminder.cancel" => {
            let wire: ReminderIdWire = decode(payload)?;
            let input = ReminderCancelInput {
                reminder_id: parse_reminder_id(&wire.reminder_id)?,
                actor,
                channel,
            };
            let out = reminder_cancel(ctx, input)
                .await
                .map_err(OpError::from_core)?;
            Ok(json!({ "reminder": out.reminder }))
        }

        // -----------------------------------------------------------------
        // Tickers
        // -----------------------------------------------------------------
        "tick.schedules" => {
            let out = schedules_tick(ctx).await.map_err(OpError::from_core)?;
            Ok(json!({
                "ran_schedule_ids": out.ran_schedule_ids,
                "drafted_invoice_ids": out
                    .drafts
                    .iter()
                    .map(|d| d.invoice.id.to_string())
                    .collect::<Vec<_>>(),
            }))
        }
        "tick.reminders" => {
            let out = reminders_tick(ctx).await.map_err(OpError::from_core)?;
            Ok(json!({ "sent_reminders": out.sent_reminders }))
        }
        "tick.overdue" => {
            let out = mark_overdue_ticker(ctx).await.map_err(OpError::from_core)?;
            Ok(json!({ "overdue_invoices": out.overdue_invoices }))
        }

        // -----------------------------------------------------------------
        // Lifecycle / liveness
        // -----------------------------------------------------------------
        "ping" => Ok(json!({ "pong": true })),

        // -----------------------------------------------------------------
        // Unknown
        // -----------------------------------------------------------------
        other => Err(OpError {
            code: "unknown_op",
            message: format!("unknown op: `{other}`"),
        }),
    }
}

/// Every op name this dispatcher recognises (used by tests / docs).
pub const OP_NAMES: &[&str] = &[
    "invoice.draft",
    "invoice.issue",
    "invoice.send",
    "invoice.pay",
    "invoice.void",
    "creditnote.draft",
    "creditnote.issue",
    "schedule.create",
    "schedule.pause",
    "schedule.cancel",
    "reminder.schedule",
    "reminder.cancel",
    "tick.schedules",
    "tick.reminders",
    "tick.overdue",
    "ping",
];

// =============================================================================
// Wire-shape payload structs
// =============================================================================

/// Wire payload for `invoice.draft`.
#[derive(Debug, Deserialize)]
struct DraftInvoiceWire {
    customer_id: String,
    seller_jurisdiction: Jurisdiction,
    currency: Currency,
    lines: Vec<DraftLineWire>,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    due_at: Option<DateTime<Utc>>,
    #[serde(default)]
    template_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DraftLineWire {
    description: String,
    quantity: Decimal,
    unit_price: Decimal,
    #[serde(default)]
    tax_category: TaxCategory,
}

impl DraftInvoiceWire {
    fn into_input(self, actor: Actor, channel: Channel) -> Result<DraftInvoiceInput, OpError> {
        let customer_id = parse_customer_id(&self.customer_id)?;
        Ok(DraftInvoiceInput {
            customer_id,
            seller_jurisdiction: self.seller_jurisdiction,
            currency: self.currency,
            lines: self
                .lines
                .into_iter()
                .map(|l| DraftLineInput {
                    description: l.description,
                    quantity: l.quantity,
                    unit_price: l.unit_price,
                    tax_category: l.tax_category,
                })
                .collect(),
            idempotency_key: self.idempotency_key,
            actor,
            channel,
            due_at: self.due_at,
            template_path: self.template_path,
            schedule_id: None,
        })
    }
}

/// Wire payload for ops that take just an invoice id + optional idempotency key.
#[derive(Debug, Deserialize)]
struct InvoiceIdWire {
    invoice_id: String,
    #[serde(default)]
    idempotency_key: Option<String>,
}

impl InvoiceIdWire {
    fn invoice_id(&self) -> Result<InvoiceId, OpError> {
        parse_invoice_id(&self.invoice_id)
    }
}

/// Wire payload for `invoice.send`.
#[derive(Debug, Deserialize)]
struct SendInvoiceWire {
    invoice_id: String,
    destination_uri: String,
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Wire payload for `invoice.pay`.
#[derive(Debug, Deserialize)]
struct MarkPaidWire {
    invoice_id: String,
    amount: Decimal,
    #[serde(default)]
    received_at: Option<DateTime<Utc>>,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    bus_event_id: Option<String>,
}

/// Wire payload for `invoice.void`.
#[derive(Debug, Deserialize)]
struct VoidInvoiceWire {
    invoice_id: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Wire payload for `creditnote.draft`.
#[derive(Debug, Deserialize)]
struct CreateCreditNoteWire {
    invoice_id: String,
    amount: Decimal,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    refund_ref: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Wire payload for `creditnote.issue`.
#[derive(Debug, Deserialize)]
struct CreditNoteIdWire {
    credit_note_id: String,
    #[serde(default)]
    idempotency_key: Option<String>,
}

/// Wire payload for `schedule.create`.
#[derive(Debug, Deserialize)]
struct ScheduleCreateWire {
    customer_id: String,
    template_lines: Vec<ScheduleLineWire>,
    currency: Currency,
    cadence: Cadence,
    start_date: NaiveDate,
    #[serde(default)]
    end_date: Option<NaiveDate>,
    #[serde(default)]
    auto_issue: bool,
}

#[derive(Debug, Deserialize)]
struct ScheduleLineWire {
    description: String,
    quantity: Decimal,
    unit_price: Decimal,
    #[serde(default)]
    tax_category: TaxCategory,
}

impl ScheduleCreateWire {
    fn into_input(self, actor: Actor, channel: Channel) -> Result<ScheduleCreateInput, OpError> {
        let customer_id = parse_customer_id(&self.customer_id)?;
        Ok(ScheduleCreateInput {
            customer_id,
            template_lines: self
                .template_lines
                .into_iter()
                .map(|l| ScheduleLineInput {
                    description: l.description,
                    quantity: l.quantity,
                    unit_price: l.unit_price,
                    tax_category: l.tax_category,
                })
                .collect(),
            currency: self.currency,
            cadence: self.cadence,
            start_date: self.start_date,
            end_date: self.end_date,
            auto_issue: self.auto_issue,
            actor,
            channel,
        })
    }
}

/// Wire payload for ops that take just a schedule id.
#[derive(Debug, Deserialize)]
struct ScheduleIdWire {
    schedule_id: String,
}

/// Wire payload for `reminder.schedule`.
#[derive(Debug, Deserialize)]
struct ReminderScheduleWire {
    invoice_id: String,
    scheduled_at: DateTime<Utc>,
    channel_scheme: ReminderChannel,
}

/// Wire payload for `reminder.cancel`.
#[derive(Debug, Deserialize)]
struct ReminderIdWire {
    reminder_id: String,
}

// =============================================================================
// Helpers
// =============================================================================

fn decode<T: for<'de> Deserialize<'de>>(v: Value) -> Result<T, OpError> {
    serde_json::from_value(v).map_err(|e| OpError::validation(format!("payload decode: {e}")))
}

fn parse_invoice_id(s: &str) -> Result<InvoiceId, OpError> {
    InvoiceId::parse(s).map_err(|e| OpError::validation(format!("invoice_id: {e}")))
}
fn parse_customer_id(s: &str) -> Result<CustomerId, OpError> {
    CustomerId::parse(s).map_err(|e| OpError::validation(format!("customer_id: {e}")))
}
fn parse_credit_note_id(s: &str) -> Result<CreditNoteId, OpError> {
    CreditNoteId::parse(s).map_err(|e| OpError::validation(format!("credit_note_id: {e}")))
}
fn parse_schedule_id(s: &str) -> Result<ScheduleId, OpError> {
    ScheduleId::parse(s).map_err(|e| OpError::validation(format!("schedule_id: {e}")))
}
fn parse_reminder_id(s: &str) -> Result<ReminderId, OpError> {
    ReminderId::parse(s).map_err(|e| OpError::validation(format!("reminder_id: {e}")))
}

// Keep one Serialize-derived stub so the `Serialize` import isn't unused if
// the file ever stops using `json!()` for response shaping.
#[allow(dead_code)]
#[derive(Serialize)]
struct _SerdeKeepAlive;
