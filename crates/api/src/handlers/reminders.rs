//! Reminder routes — schedule / cancel / list.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};

use hop_top_inv_commands::{
    reminder_cancel, reminder_schedule, Actor, Channel, ReminderCancelInput, ReminderScheduleInput,
};
use hop_top_inv_core::domain::ids::{InvoiceId, ReminderId};
use hop_top_inv_core::domain::reminder::ReminderChannel;
use hop_top_inv_store::repo::reminder::ReminderRepo;

use crate::error::ApiError;
use crate::state::ApiState;

// =============================================================================
// POST /v1/invoices/:id/reminders
// =============================================================================

/// Body for [`schedule`].
#[derive(Debug, Deserialize)]
pub struct ScheduleBody {
    /// When the ticker should dispatch (RFC 3339).
    pub scheduled_at: DateTime<Utc>,
    /// Delivery channel scheme.
    pub channel: ReminderChannel,
}

/// `POST /v1/invoices/{id}/reminders`.
pub async fn schedule(
    State(state): State<Arc<ApiState>>,
    Path(invoice_id): Path<String>,
    Json(body): Json<ScheduleBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let invoice_id: InvoiceId = invoice_id
        .parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid invoice id: {e}")))?;
    let input = ReminderScheduleInput {
        invoice_id,
        scheduled_at: body.scheduled_at,
        channel_scheme: body.channel,
        actor: Actor::Api {
            name: "http".into(),
        },
        channel: Channel::Api,
    };
    let out = reminder_schedule(&state.ctx, input).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "reminder": out.reminder,
        })),
    ))
}

// =============================================================================
// POST /v1/reminders/:id/cancel
// =============================================================================

/// `POST /v1/reminders/{id}/cancel`.
pub async fn cancel(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id: ReminderId = id
        .parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid reminder id: {e}")))?;
    let input = ReminderCancelInput {
        reminder_id: id,
        actor: Actor::Api {
            name: "http".into(),
        },
        channel: Channel::Api,
    };
    let out = reminder_cancel(&state.ctx, input).await?;
    Ok(Json(json!({
        "reminder": out.reminder,
    })))
}

// =============================================================================
// GET /v1/reminders  (?invoice_id=)
// =============================================================================

/// Query parameters for [`list`].
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    /// Restrict to reminders for a specific invoice.
    #[serde(default)]
    pub invoice_id: Option<String>,
}

/// `GET /v1/reminders`.
pub async fn list(
    State(state): State<Arc<ApiState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let repo = ReminderRepo::new(&state.ctx.db);
    let rows = match q.invoice_id.as_deref() {
        None | Some("") => Vec::new(),
        Some(s) => {
            let invoice_id: InvoiceId = s
                .parse()
                .map_err(|e| ApiError::BadRequest(format!("invalid invoice id: {e}")))?;
            repo.list_for_invoice(&invoice_id)
                .await
                .map_err(hop_top_inv_commands::CoreError::from)?
        }
    };
    Ok(Json(json!({ "reminders": rows })))
}
