//! Schedule commands: create / update / pause / cancel + materialisation ticker.
//!
//! A [`Schedule`] is a recipe for generating draft invoices on a cadence.
//! Per design §8, the ticker (typically a background job in `inv-server`)
//! calls [`schedules_tick`] daily; it materialises invoices for any
//! active schedule whose `next_run <= today`, then advances `next_run`
//! via [`Cadence::advance`].

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::json;

use inv_core::domain::ids::{CustomerId, ScheduleId};
use inv_core::domain::invoice::TaxCategory;
use inv_core::domain::money::Currency;
use inv_core::domain::schedule::{Cadence, Schedule, ScheduleLine, ScheduleState};
use inv_store::repo::schedule::ScheduleRepo;

use crate::ctx::{Actor, Channel, CoreCtx};
use crate::draft::{
    draft_invoice, history_channel_str, DraftInvoiceInput, DraftInvoiceOutput, DraftLineInput,
};
use crate::error::CoreError;
use crate::events::EmittedEvent;
use crate::issue::{issue_invoice, IssueInvoiceInput};
use inv_core::domain::ids::LineId;

// =============================================================================
// schedule_create
// =============================================================================

/// Input for [`schedule_create`].
#[derive(Debug, Clone)]
pub struct ScheduleCreateInput {
    /// Customer to bill on every cycle.
    pub customer_id: CustomerId,
    /// Line templates instantiated each cycle.
    pub template_lines: Vec<ScheduleLineInput>,
    /// Currency for materialised invoices.
    pub currency: Currency,
    /// Cadence (monthly@N | quarterly@N | yearly@MM-DD).
    pub cadence: Cadence,
    /// First cycle date.
    pub start_date: NaiveDate,
    /// Optional last cycle date.
    pub end_date: Option<NaiveDate>,
    /// If true, materialised drafts auto-transition to Issued.
    pub auto_issue: bool,
    /// Who triggered.
    pub actor: Actor,
    /// Channel.
    pub channel: Channel,
}

/// One template line.
#[derive(Debug, Clone)]
pub struct ScheduleLineInput {
    /// Free-form description.
    pub description: String,
    /// Quantity.
    pub quantity: Decimal,
    /// Per-unit price.
    pub unit_price: Decimal,
    /// Tax category override.
    pub tax_category: TaxCategory,
}

impl ScheduleCreateInput {
    /// Validate: non-empty lines, positive qty/price.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.template_lines.is_empty() {
            return Err(CoreError::Validation(
                "schedule must have at least one template line".into(),
            ));
        }
        for (i, l) in self.template_lines.iter().enumerate() {
            if l.quantity <= Decimal::ZERO {
                return Err(CoreError::Validation(format!(
                    "line {i}: quantity must be > 0"
                )));
            }
            if l.unit_price < Decimal::ZERO {
                return Err(CoreError::Validation(format!(
                    "line {i}: unit_price must be >= 0"
                )));
            }
        }
        if let Some(end) = self.end_date {
            if end < self.start_date {
                return Err(CoreError::Validation(
                    "end_date must be >= start_date".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Output of [`schedule_create`].
#[derive(Debug, Clone)]
pub struct ScheduleCreateOutput {
    /// The newly-created schedule.
    pub schedule: Schedule,
    /// Bus events the command would emit.
    pub emitted_events: Vec<EmittedEvent>,
}

/// Create an active recurring schedule.
#[tracing::instrument(skip_all, fields(customer_id = %input.customer_id))]
pub async fn schedule_create(
    ctx: &CoreCtx,
    input: ScheduleCreateInput,
) -> Result<ScheduleCreateOutput, CoreError> {
    input.validate()?;

    let now = ctx.clock.now();
    let template_lines: Vec<ScheduleLine> = input
        .template_lines
        .into_iter()
        .map(|l| ScheduleLine {
            description: l.description,
            quantity: l.quantity,
            unit_price: l.unit_price,
            tax_category: l.tax_category,
            id: LineId::new(),
        })
        .collect();

    let schedule = Schedule {
        id: ScheduleId::new(),
        customer_id: input.customer_id.clone(),
        template_lines,
        currency: input.currency,
        cadence: input.cadence,
        start_date: input.start_date,
        end_date: input.end_date,
        auto_issue: input.auto_issue,
        next_run: input.start_date,
        last_run: None,
        state: ScheduleState::Active,
        metadata: BTreeMap::new(),
        created_at: now,
        updated_at: now,
    };

    ScheduleRepo::new(&ctx.db).save(&schedule).await?;

    let emitted = vec![EmittedEvent::new(
        "inv.billing.schedule.created",
        json!({
            "schedule_id": schedule.id.to_string(),
            "customer_id": schedule.customer_id.to_string(),
            "cadence": schedule.cadence.to_string(),
            "currency": schedule.currency.to_string(),
            "auto_issue": schedule.auto_issue,
            "start_date": schedule.start_date.to_string(),
            "actor": input.actor.audit_string(),
            "channel": history_channel_str(input.channel.into()),
        }),
        now,
    )];

    Ok(ScheduleCreateOutput {
        schedule,
        emitted_events: emitted,
    })
}

// =============================================================================
// schedule_pause / schedule_cancel
// =============================================================================

/// Input for [`schedule_pause`] / [`schedule_cancel`].
#[derive(Debug, Clone)]
pub struct ScheduleStateChangeInput {
    /// Schedule to transition.
    pub schedule_id: ScheduleId,
    /// Who triggered.
    pub actor: Actor,
    /// Channel.
    pub channel: Channel,
}

/// Output of [`schedule_pause`] / [`schedule_cancel`].
#[derive(Debug, Clone)]
pub struct ScheduleStateChangeOutput {
    /// Updated schedule.
    pub schedule: Schedule,
    /// Bus events.
    pub emitted_events: Vec<EmittedEvent>,
}

/// Pause an active schedule. Idempotent: pausing a paused schedule succeeds.
#[tracing::instrument(skip_all, fields(schedule_id = %input.schedule_id))]
pub async fn schedule_pause(
    ctx: &CoreCtx,
    input: ScheduleStateChangeInput,
) -> Result<ScheduleStateChangeOutput, CoreError> {
    transition_schedule(ctx, input, ScheduleState::Paused, "paused").await
}

/// Cancel a schedule. Idempotent: cancelling a cancelled schedule succeeds.
/// Cancelled is terminal.
#[tracing::instrument(skip_all, fields(schedule_id = %input.schedule_id))]
pub async fn schedule_cancel(
    ctx: &CoreCtx,
    input: ScheduleStateChangeInput,
) -> Result<ScheduleStateChangeOutput, CoreError> {
    transition_schedule(ctx, input, ScheduleState::Cancelled, "cancelled").await
}

async fn transition_schedule(
    ctx: &CoreCtx,
    input: ScheduleStateChangeInput,
    to_state: ScheduleState,
    event_tag: &'static str,
) -> Result<ScheduleStateChangeOutput, CoreError> {
    let repo = ScheduleRepo::new(&ctx.db);
    let mut schedule = repo
        .get(&input.schedule_id)
        .await?
        .ok_or_else(|| CoreError::NotFound(format!("schedule {}", input.schedule_id)))?;

    // Reject cancelled→anything (terminal).
    if matches!(schedule.state, ScheduleState::Cancelled) && to_state != ScheduleState::Cancelled {
        return Err(CoreError::Validation(format!(
            "schedule {} is cancelled (terminal); cannot transition to {to_state:?}",
            input.schedule_id
        )));
    }

    let now = ctx.clock.now();
    schedule.state = to_state;
    schedule.updated_at = now;
    repo.save(&schedule).await?;

    let topic = format!("inv.billing.schedule.{event_tag}");
    let emitted = vec![EmittedEvent::new(
        topic,
        json!({
            "schedule_id": schedule.id.to_string(),
            "state": format!("{:?}", schedule.state).to_lowercase(),
            "actor": input.actor.audit_string(),
            "channel": history_channel_str(input.channel.into()),
        }),
        now,
    )];

    Ok(ScheduleStateChangeOutput {
        schedule,
        emitted_events: emitted,
    })
}

// =============================================================================
// schedules_tick
// =============================================================================

/// Output of [`schedules_tick`].
#[derive(Debug, Clone)]
pub struct SchedulesTickOutput {
    /// Schedule IDs that ran on this tick (i.e., materialised a draft).
    pub ran_schedule_ids: Vec<ScheduleId>,
    /// Outputs of every draft (and issue, when auto_issue) call made.
    pub drafts: Vec<DraftInvoiceOutput>,
    /// All bus events emitted across the tick.
    pub emitted_events: Vec<EmittedEvent>,
}

/// Materialise invoices for every active schedule with `next_run <= today`.
///
/// Runs the schedule ticker per design §8:
/// 1. For each due schedule:
///    a. Build a `DraftInvoiceInput` from the template lines.
///    b. Call [`draft_invoice`] to create + persist a draft.
///    c. If `auto_issue`, call [`issue_invoice`].
///    d. Advance `next_run` via [`Cadence::advance`] and update `last_run`.
/// 2. If the new `next_run > end_date`, mark the schedule `Cancelled`.
#[tracing::instrument(skip_all)]
pub async fn schedules_tick(ctx: &CoreCtx) -> Result<SchedulesTickOutput, CoreError> {
    let now = ctx.clock.now();
    let today = now.date_naive();

    let repo = ScheduleRepo::new(&ctx.db);
    let due = repo.due(today).await?;

    let mut ran = Vec::new();
    let mut drafts = Vec::new();
    let mut events = Vec::new();

    for mut schedule in due {
        if !matches!(schedule.state, ScheduleState::Active) {
            continue;
        }

        // Build a draft from the template lines.
        let lines: Vec<DraftLineInput> = schedule
            .template_lines
            .iter()
            .map(|l| DraftLineInput {
                description: l.description.clone(),
                quantity: l.quantity,
                unit_price: l.unit_price,
                tax_category: l.tax_category,
            })
            .collect();
        let draft_input = DraftInvoiceInput {
            customer_id: schedule.customer_id.clone(),
            seller_jurisdiction: inv_core::domain::jurisdiction::Jurisdiction::QuebecCa,
            // TODO(T-0014): the schedule should carry the seller_jurisdiction.
            // For v1 we default to QuebecCa to keep tests + the default tax
            // table coherent. The schedule-create command will gain a
            // seller_jurisdiction field in the same task that wires bus pubs.
            currency: schedule.currency,
            lines,
            idempotency_key: Some(format!(
                "schedule:{}:run:{}",
                schedule.id, schedule.next_run
            )),
            actor: Actor::Bus {
                source: "inv.scheduler".into(),
            },
            channel: Channel::Bus,
            due_at: None,
            template_path: None,
            schedule_id: Some(schedule.id.clone()),
        };
        let drafted = draft_invoice(ctx, draft_input).await?;
        events.extend(drafted.emitted_events.iter().cloned());

        if schedule.auto_issue {
            let issued = issue_invoice(
                ctx,
                IssueInvoiceInput {
                    invoice_id: drafted.invoice.id.clone(),
                    idempotency_key: None,
                    actor: Actor::Bus {
                        source: "inv.scheduler".into(),
                    },
                    channel: Channel::Bus,
                },
            )
            .await?;
            events.extend(issued.emitted_events);
        }

        // Advance schedule.
        schedule.last_run = Some(schedule.next_run);
        let next = schedule.cadence.advance(schedule.next_run);
        schedule.next_run = next;
        schedule.updated_at = now;
        if let Some(end) = schedule.end_date {
            if next > end {
                schedule.state = ScheduleState::Cancelled;
                events.push(EmittedEvent::new(
                    "inv.billing.schedule.cancelled",
                    json!({
                        "schedule_id": schedule.id.to_string(),
                        "state": "cancelled",
                        "reason": "end_date_reached",
                    }),
                    now,
                ));
            }
        }
        repo.save(&schedule).await?;

        ran.push(schedule.id.clone());
        drafts.push(drafted);
    }

    Ok(SchedulesTickOutput {
        ran_schedule_ids: ran,
        drafts,
        emitted_events: events,
    })
}

// Silence DateTime<Utc>-import warning if not used.
const _: Option<DateTime<Utc>> = None;
