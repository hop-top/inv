//! Reminder commands: schedule / cancel + dispatch ticker.
//!
//! Reminders do not advance the invoice FSM. The act of reminding is its
//! own event class (`inv.billing.reminder.scheduled` / `.sent`).

use chrono::{DateTime, Utc};
use serde_json::json;

use hop_top_inv_core::domain::ids::{InvoiceId, ReminderId};
use hop_top_inv_core::domain::reminder::{Reminder, ReminderChannel, ReminderState};
use hop_top_inv_store::repo::reminder::ReminderRepo;

use crate::ctx::{Actor, Channel, CoreCtx};
use crate::draft::history_channel_str;
use crate::error::CoreError;
use crate::publisher::try_publish;

// =============================================================================
// reminder_schedule
// =============================================================================

/// Input for [`reminder_schedule`].
#[derive(Debug, Clone)]
pub struct ReminderScheduleInput {
    /// Invoice the reminder targets.
    pub invoice_id: InvoiceId,
    /// When the ticker should dispatch.
    pub scheduled_at: DateTime<Utc>,
    /// Delivery channel scheme.
    pub channel_scheme: ReminderChannel,
    /// Who triggered.
    pub actor: Actor,
    /// Channel the request arrived on.
    pub channel: Channel,
}

impl ReminderScheduleInput {
    /// Validate: scheduled_at must be in the future (per `ctx.clock.now()`).
    pub fn validate(&self, now: DateTime<Utc>) -> Result<(), CoreError> {
        if self.scheduled_at <= now {
            return Err(CoreError::Validation(format!(
                "scheduled_at must be in the future; got {} (now: {})",
                self.scheduled_at, now
            )));
        }
        Ok(())
    }
}

/// Output of [`reminder_schedule`].
#[derive(Debug, Clone)]
pub struct ReminderScheduleOutput {
    /// The newly-enqueued reminder.
    pub reminder: Reminder,
}

/// Enqueue a reminder against an invoice.
#[tracing::instrument(skip_all, fields(invoice_id = %input.invoice_id))]
pub async fn reminder_schedule(
    ctx: &CoreCtx,
    input: ReminderScheduleInput,
) -> Result<ReminderScheduleOutput, CoreError> {
    let now = ctx.clock.now();
    input.validate(now)?;

    let reminder = Reminder {
        id: ReminderId::new(),
        invoice_id: input.invoice_id.clone(),
        scheduled_at: input.scheduled_at,
        sent_at: None,
        channel: input.channel_scheme,
        state: ReminderState::Scheduled,
    };

    ReminderRepo::new(&ctx.db).save(&reminder).await?;

    try_publish(
        ctx,
        "inv.billing.reminder.scheduled",
        json!({
            "reminder_id": reminder.id.to_string(),
            "invoice_id": reminder.invoice_id.to_string(),
            "scheduled_at": reminder.scheduled_at.to_rfc3339(),
            "channel": format!("{:?}", reminder.channel).to_lowercase(),
            "actor": input.actor.audit_string(),
            "history_channel": history_channel_str(input.channel.into()),
        }),
        now,
    )
    .await;

    Ok(ReminderScheduleOutput { reminder })
}

// =============================================================================
// reminder_cancel
// =============================================================================

/// Input for [`reminder_cancel`].
#[derive(Debug, Clone)]
pub struct ReminderCancelInput {
    /// Reminder to cancel.
    pub reminder_id: ReminderId,
    /// Who triggered.
    pub actor: Actor,
    /// Channel.
    pub channel: Channel,
}

/// Output of [`reminder_cancel`].
#[derive(Debug, Clone)]
pub struct ReminderCancelOutput {
    /// The cancelled reminder.
    pub reminder: Reminder,
}

/// Cancel a scheduled reminder. No-op if already sent or cancelled.
#[tracing::instrument(skip_all, fields(reminder_id = %input.reminder_id))]
pub async fn reminder_cancel(
    ctx: &CoreCtx,
    input: ReminderCancelInput,
) -> Result<ReminderCancelOutput, CoreError> {
    let repo = ReminderRepo::new(&ctx.db);
    let mut reminder = repo
        .get(&input.reminder_id)
        .await?
        .ok_or_else(|| CoreError::NotFound(format!("reminder {}", input.reminder_id)))?;

    if matches!(reminder.state, ReminderState::Scheduled) {
        reminder.state = ReminderState::Cancelled;
        repo.save(&reminder).await?;
    }

    let now = ctx.clock.now();
    try_publish(
        ctx,
        "inv.billing.reminder.cancelled",
        json!({
            "reminder_id": reminder.id.to_string(),
            "invoice_id": reminder.invoice_id.to_string(),
            "actor": input.actor.audit_string(),
            "channel": history_channel_str(input.channel.into()),
        }),
        now,
    )
    .await;

    Ok(ReminderCancelOutput { reminder })
}

// =============================================================================
// reminders_tick
// =============================================================================

/// Output of [`reminders_tick`].
#[derive(Debug, Clone)]
pub struct RemindersTickOutput {
    /// Reminders that were dispatched on this tick.
    pub sent_reminders: Vec<Reminder>,
}

/// Dispatch every reminder whose `scheduled_at <= now` and is still in
/// `Scheduled` state.
///
/// v1 does NOT perform actual delivery here — it just transitions the
/// reminder to `Sent` and emits `inv.billing.reminder.sent`. The
/// downstream consumer (a bus-consumer adapter at T-0015 or the
/// inv-server compositor at T-0020) is responsible for translating the
/// emitted event into a real delivery call (write file, POST webhook,
/// etc.). This separation keeps the command layer pure.
#[tracing::instrument(skip_all)]
pub async fn reminders_tick(ctx: &CoreCtx) -> Result<RemindersTickOutput, CoreError> {
    let now = ctx.clock.now();
    let repo = ReminderRepo::new(&ctx.db);
    let due = repo.due(now).await?;

    let mut sent = Vec::new();
    for mut reminder in due {
        if !matches!(reminder.state, ReminderState::Scheduled) {
            continue;
        }
        reminder.state = ReminderState::Sent;
        reminder.sent_at = Some(now);
        repo.save(&reminder).await?;

        try_publish(
            ctx,
            "inv.billing.reminder.sent",
            json!({
                "reminder_id": reminder.id.to_string(),
                "invoice_id": reminder.invoice_id.to_string(),
                "channel": format!("{:?}", reminder.channel).to_lowercase(),
                "sent_at": now.to_rfc3339(),
            }),
            now,
        )
        .await;
        sent.push(reminder);
    }

    Ok(RemindersTickOutput {
        sent_reminders: sent,
    })
}
