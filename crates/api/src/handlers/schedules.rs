//! Schedule routes — create / list / get / pause / cancel.

use std::str::FromStr;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};

use inv_commands::{
    schedule_cancel, schedule_create, schedule_pause, Actor, Channel,
    ScheduleCreateInput, ScheduleLineInput, ScheduleStateChangeInput,
};
use inv_core::domain::ids::{CustomerId, ScheduleId};
use inv_core::domain::invoice::TaxCategory;
use inv_core::domain::money::Currency;
use inv_core::domain::schedule::{Cadence, ScheduleState};
use inv_store::repo::schedule::{ScheduleFilter, ScheduleRepo};

use crate::error::ApiError;
use crate::state::ApiState;

// =============================================================================
// POST /v1/schedules
// =============================================================================

/// Body for [`create`].
#[derive(Debug, Deserialize)]
pub struct CreateBody {
    /// Customer to bill on every cycle.
    pub customer_id: String,
    /// Template line items.
    pub template_lines: Vec<TemplateLineBody>,
    /// Currency.
    pub currency: String,
    /// Cadence string (`monthly@1`, `quarterly@15`, `yearly@01-01`).
    pub cadence: String,
    /// First cycle date (YYYY-MM-DD).
    pub start_date: NaiveDate,
    /// Optional last cycle date.
    #[serde(default)]
    pub end_date: Option<NaiveDate>,
    /// If true, materialised drafts auto-transition to Issued.
    #[serde(default)]
    pub auto_issue: bool,
}

/// One template line.
#[derive(Debug, Deserialize)]
pub struct TemplateLineBody {
    /// Description.
    pub description: String,
    /// Quantity.
    pub quantity: Decimal,
    /// Per-unit price.
    pub unit_price: Decimal,
    /// Tax category. Defaults to `standard`.
    #[serde(default)]
    pub tax_category: TaxCategory,
}

/// `POST /v1/schedules`.
pub async fn create(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let customer_id: CustomerId = body
        .customer_id
        .parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid customer id: {e}")))?;
    let currency: Currency = body
        .currency
        .parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid currency: {e}")))?;
    let cadence = Cadence::from_str(&body.cadence)
        .map_err(|e| ApiError::BadRequest(format!("invalid cadence: {e}")))?;
    let template_lines = body
        .template_lines
        .into_iter()
        .map(|l| ScheduleLineInput {
            description: l.description,
            quantity: l.quantity,
            unit_price: l.unit_price,
            tax_category: l.tax_category,
        })
        .collect();
    let input = ScheduleCreateInput {
        customer_id,
        template_lines,
        currency,
        cadence,
        start_date: body.start_date,
        end_date: body.end_date,
        auto_issue: body.auto_issue,
        actor: Actor::Api { name: "http".into() },
        channel: Channel::Api,
    };
    let out = schedule_create(&state.ctx, input).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "schedule": out.schedule,
            "emitted_events": out.emitted_events,
        })),
    ))
}

// =============================================================================
// POST /v1/schedules/:id/pause | /v1/schedules/:id/cancel
// =============================================================================

/// `POST /v1/schedules/{id}/pause`.
pub async fn pause(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id: ScheduleId = id
        .parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid schedule id: {e}")))?;
    let input = ScheduleStateChangeInput {
        schedule_id: id,
        actor: Actor::Api { name: "http".into() },
        channel: Channel::Api,
    };
    let out = schedule_pause(&state.ctx, input).await?;
    Ok(Json(json!({
        "schedule": out.schedule,
        "emitted_events": out.emitted_events,
    })))
}

/// `POST /v1/schedules/{id}/cancel`.
pub async fn cancel(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id: ScheduleId = id
        .parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid schedule id: {e}")))?;
    let input = ScheduleStateChangeInput {
        schedule_id: id,
        actor: Actor::Api { name: "http".into() },
        channel: Channel::Api,
    };
    let out = schedule_cancel(&state.ctx, input).await?;
    Ok(Json(json!({
        "schedule": out.schedule,
        "emitted_events": out.emitted_events,
    })))
}

// =============================================================================
// GET /v1/schedules/:id
// =============================================================================

/// `GET /v1/schedules/{id}`.
pub async fn get(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id: ScheduleId = id
        .parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid schedule id: {e}")))?;
    let s = ScheduleRepo::new(&state.ctx.db)
        .get(&id)
        .await
        .map_err(inv_commands::CoreError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("schedule {id}")))?;
    Ok(Json(json!({ "schedule": s })))
}

// =============================================================================
// GET /v1/schedules  (?customer_id=&state=&limit=&offset=)
// =============================================================================

/// Query parameters for [`list`].
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    /// Restrict to schedules for a specific customer.
    #[serde(default)]
    pub customer_id: Option<String>,
    /// Restrict to a specific lifecycle state (`active`, `paused`, `cancelled`).
    #[serde(default)]
    pub state: Option<String>,
    /// Max rows.
    #[serde(default)]
    pub limit: Option<i64>,
    /// Offset for paging.
    #[serde(default)]
    pub offset: Option<i64>,
}

/// `GET /v1/schedules`.
pub async fn list(
    State(state): State<Arc<ApiState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let customer_id = match q.customer_id.as_deref() {
        None | Some("") => None,
        Some(s) => Some(
            s.parse::<CustomerId>()
                .map_err(|e| ApiError::BadRequest(format!("invalid customer id: {e}")))?,
        ),
    };
    let sched_state = match q.state.as_deref() {
        None | Some("") => None,
        Some(s) => Some(parse_schedule_state(s)?),
    };
    let filter = ScheduleFilter {
        customer_id,
        state: sched_state,
        limit: q.limit,
        offset: q.offset,
    };
    let rows = ScheduleRepo::new(&state.ctx.db)
        .list(&filter)
        .await
        .map_err(inv_commands::CoreError::from)?;
    Ok(Json(json!({ "schedules": rows })))
}

fn parse_schedule_state(s: &str) -> Result<ScheduleState, ApiError> {
    match s {
        "active" => Ok(ScheduleState::Active),
        "paused" => Ok(ScheduleState::Paused),
        "cancelled" => Ok(ScheduleState::Cancelled),
        other => Err(ApiError::BadRequest(format!(
            "invalid schedule state `{other}` (want active | paused | cancelled)"
        ))),
    }
}
