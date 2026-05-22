//! Ticker tools — drain the recurring/notification queues.
//!
//! - `inv_tick_schedules` → [`inv_commands::schedules_tick`]
//! - `inv_tick_reminders` → [`inv_commands::reminders_tick`]
//! - `inv_tick_overdue`   → [`inv_commands::mark_overdue_ticker`]

use inv_commands::{mark_overdue_ticker, reminders_tick, schedules_tick, CoreCtx};

use crate::error::McpError;

/// Run the schedules ticker. The full draft outputs aren't ferried
/// over MCP (they reference draft state already accessible via
/// `inv_invoice_show`); we surface the count + ran ids + emitted
/// events as a structured summary.
pub async fn tick_schedules(ctx: &CoreCtx) -> Result<serde_json::Value, McpError> {
    let out = schedules_tick(ctx).await?;
    crate::tools::common::to_value(&serde_json::json!({
        "ran_schedule_ids": out.ran_schedule_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
        "drafts_count": out.drafts.len(),
        "emitted_events": out.emitted_events,
    }))
}

/// Run the reminders ticker.
pub async fn tick_reminders(ctx: &CoreCtx) -> Result<serde_json::Value, McpError> {
    let out = reminders_tick(ctx).await?;
    crate::tools::common::to_value(&serde_json::json!({
        "sent_reminders": out.sent_reminders,
        "emitted_events": out.emitted_events,
    }))
}

/// Run the overdue ticker.
pub async fn tick_overdue(ctx: &CoreCtx) -> Result<serde_json::Value, McpError> {
    let out = mark_overdue_ticker(ctx).await?;
    crate::tools::common::to_value(&serde_json::json!({
        "overdue_invoices": out.overdue_invoices,
        "emitted_events": out.emitted_events,
    }))
}
