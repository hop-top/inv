//! Reminder tools.
//!
//! - `inv_reminder_schedule` → [`inv_commands::reminder_schedule`]
//! - `inv_reminder_cancel`   → [`inv_commands::reminder_cancel`]

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use inv_commands::{
    reminder_cancel, reminder_schedule, CoreCtx, ReminderCancelInput, ReminderScheduleInput,
};
use inv_core::domain::ids::{InvoiceId, ReminderId};
use inv_core::domain::reminder::ReminderChannel;

use crate::error::McpError;
use crate::tools::common::{mcp_actor, mcp_channel};

/// Input for `inv_reminder_schedule`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReminderScheduleWire {
    /// Invoice the reminder targets.
    pub invoice_id: String,
    /// When the ticker should dispatch (must be in the future).
    pub scheduled_at: DateTime<Utc>,
    /// Delivery channel scheme (`file`, `stdout`, `bus`, `webhook`, `link`).
    pub channel_scheme: ReminderChannel,
}

/// Schedule a reminder.
pub async fn schedule(
    ctx: &CoreCtx,
    input: ReminderScheduleWire,
) -> Result<serde_json::Value, McpError> {
    let invoice_id: InvoiceId = input
        .invoice_id
        .parse()
        .map_err(|e: inv_core::domain::ids::IdError| {
            McpError::Decode(format!("invoice_id: {e}"))
        })?;
    let req = ReminderScheduleInput {
        invoice_id,
        scheduled_at: input.scheduled_at,
        channel_scheme: input.channel_scheme,
        actor: mcp_actor(),
        channel: mcp_channel(),
    };
    let out = reminder_schedule(ctx, req).await?;
    crate::tools::common::to_value(&out.reminder)
}

/// Input for `inv_reminder_cancel`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReminderCancelWire {
    /// Reminder id.
    pub reminder_id: String,
}

/// Cancel a reminder.
pub async fn cancel(
    ctx: &CoreCtx,
    input: ReminderCancelWire,
) -> Result<serde_json::Value, McpError> {
    let reminder_id: ReminderId = input
        .reminder_id
        .parse()
        .map_err(|e: inv_core::domain::ids::IdError| {
            McpError::Decode(format!("reminder_id: {e}"))
        })?;
    let req = ReminderCancelInput {
        reminder_id,
        actor: mcp_actor(),
        channel: mcp_channel(),
    };
    let out = reminder_cancel(ctx, req).await?;
    crate::tools::common::to_value(&out.reminder)
}
