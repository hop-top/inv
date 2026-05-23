//! Ticker tools — drain the recurring/notification queues.
//!
//! - `inv_tick_schedules` → [`hop_top_inv_commands::schedules_tick`]
//! - `inv_tick_reminders` → [`hop_top_inv_commands::reminders_tick`]
//! - `inv_tick_overdue`   → [`hop_top_inv_commands::mark_overdue_ticker`]
//!
//! T-0043 dropped the per-tool `emitted_events` field: tickers publish
//! every event via `ctx.publisher`, so subscribers (and the outbox
//! relay) are the source of truth for event history.

use hop_top_inv_commands::{mark_overdue_ticker, reminders_tick, schedules_tick, CoreCtx};

use crate::error::McpError;

/// Run the schedules ticker. The full draft outputs aren't ferried
/// over MCP (they reference draft state already accessible via
/// `inv_invoice_show`); we surface the count + ran ids as a structured
/// summary.
pub async fn tick_schedules(ctx: &CoreCtx) -> Result<serde_json::Value, McpError> {
    let out = schedules_tick(ctx).await?;
    crate::tools::common::to_value(&serde_json::json!({
        "ran_schedule_ids": out.ran_schedule_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
        "drafts_count": out.drafts.len(),
    }))
}

/// Run the reminders ticker.
pub async fn tick_reminders(ctx: &CoreCtx) -> Result<serde_json::Value, McpError> {
    let out = reminders_tick(ctx).await?;
    crate::tools::common::to_value(&serde_json::json!({
        "sent_reminders": out.sent_reminders,
    }))
}

/// Run the overdue ticker.
pub async fn tick_overdue(ctx: &CoreCtx) -> Result<serde_json::Value, McpError> {
    let out = mark_overdue_ticker(ctx).await?;
    crate::tools::common::to_value(&serde_json::json!({
        "overdue_invoices": out.overdue_invoices,
    }))
}
