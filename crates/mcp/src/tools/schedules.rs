//! Recurring-schedule tools.
//!
//! - `inv_schedule_create` → [`inv_commands::schedule_create`]
//! - `inv_schedule_pause`  → [`inv_commands::schedule_pause`]
//! - `inv_schedule_cancel` → [`inv_commands::schedule_cancel`]

use chrono::NaiveDate;
use rust_decimal::Decimal;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use inv_commands::{
    schedule_cancel, schedule_create, schedule_pause, CoreCtx, ScheduleCreateInput,
    ScheduleLineInput, ScheduleStateChangeInput,
};
use inv_core::domain::ids::{CustomerId, ScheduleId};
use inv_core::domain::invoice::TaxCategory;
use inv_core::domain::money::Currency;
use inv_core::domain::schedule::Cadence;

use crate::error::McpError;
use crate::tools::common::{mcp_actor, mcp_channel};

/// Wire-form schedule template line.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleLineWire {
    /// Description.
    pub description: String,
    /// Quantity. Decimal-string.
    #[schemars(with = "String")]
    pub quantity: Decimal,
    /// Per-unit price. Decimal-string.
    #[schemars(with = "String")]
    pub unit_price: Decimal,
    /// Tax-category override.
    #[serde(default)]
    pub tax_category: Option<TaxCategory>,
}

/// Input for `inv_schedule_create`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleCreateWire {
    /// Customer billed every cycle.
    pub customer_id: String,
    /// At least one template line.
    pub template_lines: Vec<ScheduleLineWire>,
    /// ISO 4217 currency code.
    pub currency: String,
    /// Cadence string (`monthly@<dom>` | `quarterly@<dom>` | `yearly@MM-DD`).
    pub cadence: String,
    /// First cycle date (ISO 8601 calendar date).
    pub start_date: NaiveDate,
    /// Optional last cycle date.
    #[serde(default)]
    pub end_date: Option<NaiveDate>,
    /// If true, materialised drafts auto-transition to Issued.
    #[serde(default)]
    pub auto_issue: bool,
}

/// Create a schedule.
pub async fn create(
    ctx: &CoreCtx,
    input: ScheduleCreateWire,
) -> Result<serde_json::Value, McpError> {
    let customer_id: CustomerId = input
        .customer_id
        .parse()
        .map_err(|e: inv_core::domain::ids::IdError| {
            McpError::Decode(format!("customer_id: {e}"))
        })?;
    let currency = Currency::new(&input.currency)
        .map_err(|e| McpError::Decode(format!("currency: {e}")))?;
    let cadence: Cadence = input
        .cadence
        .parse()
        .map_err(|e: inv_core::domain::schedule::CadenceError| {
            McpError::Decode(format!("cadence: {e}"))
        })?;
    let template_lines = input
        .template_lines
        .into_iter()
        .map(|l| ScheduleLineInput {
            description: l.description,
            quantity: l.quantity,
            unit_price: l.unit_price,
            tax_category: l.tax_category.unwrap_or_default(),
        })
        .collect();
    let req = ScheduleCreateInput {
        customer_id,
        template_lines,
        currency,
        cadence,
        start_date: input.start_date,
        end_date: input.end_date,
        auto_issue: input.auto_issue,
        actor: mcp_actor(),
        channel: mcp_channel(),
    };
    let out = schedule_create(ctx, req).await?;
    crate::tools::common::to_value(&out.schedule)
}

/// Input for `inv_schedule_pause` / `inv_schedule_cancel`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleStateChangeWire {
    /// Schedule id.
    pub schedule_id: String,
}

/// Pause a schedule.
pub async fn pause(
    ctx: &CoreCtx,
    input: ScheduleStateChangeWire,
) -> Result<serde_json::Value, McpError> {
    let id: ScheduleId = input
        .schedule_id
        .parse()
        .map_err(|e: inv_core::domain::ids::IdError| {
            McpError::Decode(format!("schedule_id: {e}"))
        })?;
    let req = ScheduleStateChangeInput {
        schedule_id: id,
        actor: mcp_actor(),
        channel: mcp_channel(),
    };
    let out = schedule_pause(ctx, req).await?;
    crate::tools::common::to_value(&out.schedule)
}

/// Cancel a schedule.
pub async fn cancel(
    ctx: &CoreCtx,
    input: ScheduleStateChangeWire,
) -> Result<serde_json::Value, McpError> {
    let id: ScheduleId = input
        .schedule_id
        .parse()
        .map_err(|e: inv_core::domain::ids::IdError| {
            McpError::Decode(format!("schedule_id: {e}"))
        })?;
    let req = ScheduleStateChangeInput {
        schedule_id: id,
        actor: mcp_actor(),
        channel: mcp_channel(),
    };
    let out = schedule_cancel(ctx, req).await?;
    crate::tools::common::to_value(&out.schedule)
}
