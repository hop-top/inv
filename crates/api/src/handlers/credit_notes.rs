//! Credit-note routes.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{json, Value};

use inv_commands::{
    create_credit_note, issue_credit_note, Actor, Channel, CreateCreditNoteInput,
    IssueCreditNoteInput,
};
use inv_core::domain::creditnote::CreditNoteState;
use inv_core::domain::ids::{CreditNoteId, InvoiceId};
use inv_store::repo::credit_note::{CreditNoteFilter, CreditNoteRepo};

use crate::error::ApiError;
use crate::state::ApiState;

// =============================================================================
// POST /v1/invoices/:id/credit-notes
// =============================================================================

/// Body for [`draft`].
#[derive(Debug, Deserialize)]
pub struct DraftCreditBody {
    /// Credit amount (positive).
    pub amount: Decimal,
    /// Free-form reason.
    #[serde(default)]
    pub reason: Option<String>,
    /// Refund event id (when auto-created from a refund bus event).
    #[serde(default)]
    pub refund_ref: Option<String>,
    /// Caller-supplied dedupe key.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// `POST /v1/invoices/{id}/credit-notes`.
pub async fn draft(
    State(state): State<Arc<ApiState>>,
    Path(invoice_id): Path<String>,
    Json(body): Json<DraftCreditBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let invoice_id: InvoiceId = invoice_id
        .parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid invoice id: {e}")))?;
    let input = CreateCreditNoteInput {
        invoice_id,
        amount: body.amount,
        reason: body.reason,
        refund_ref: body.refund_ref,
        idempotency_key: body.idempotency_key,
        actor: Actor::Api {
            name: "http".into(),
        },
        channel: Channel::Api,
    };
    let out = create_credit_note(&state.ctx, input).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "credit_note": out.credit_note,
        })),
    ))
}

// =============================================================================
// POST /v1/credit-notes/:id/issue
// =============================================================================

/// `POST /v1/credit-notes/{id}/issue`.
pub async fn issue(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id: CreditNoteId = id
        .parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid credit-note id: {e}")))?;
    let input = IssueCreditNoteInput {
        credit_note_id: id,
        idempotency_key: None,
        actor: Actor::Api {
            name: "http".into(),
        },
        channel: Channel::Api,
    };
    let out = issue_credit_note(&state.ctx, input).await?;
    Ok(Json(json!({
        "credit_note": out.credit_note,
    })))
}

// =============================================================================
// GET /v1/credit-notes/:id
// =============================================================================

/// `GET /v1/credit-notes/{id}`.
pub async fn get(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id: CreditNoteId = id
        .parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid credit-note id: {e}")))?;
    let cn = CreditNoteRepo::new(&state.ctx.db)
        .get(&id)
        .await
        .map_err(inv_commands::CoreError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("credit note {id}")))?;
    Ok(Json(json!({ "credit_note": cn })))
}

// =============================================================================
// GET /v1/credit-notes  (?invoice_id=&state=&limit=&offset=)
// =============================================================================

/// Query parameters for [`list`].
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    /// Restrict to credit notes against a specific invoice.
    #[serde(default)]
    pub invoice_id: Option<String>,
    /// Restrict to a specific lifecycle state (`draft`, `issued`).
    #[serde(default)]
    pub state: Option<String>,
    /// Max rows.
    #[serde(default)]
    pub limit: Option<i64>,
    /// Offset for paging.
    #[serde(default)]
    pub offset: Option<i64>,
}

/// `GET /v1/credit-notes`.
pub async fn list(
    State(state): State<Arc<ApiState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let invoice_id = match q.invoice_id.as_deref() {
        None | Some("") => None,
        Some(s) => Some(
            s.parse::<InvoiceId>()
                .map_err(|e| ApiError::BadRequest(format!("invalid invoice id: {e}")))?,
        ),
    };
    let cn_state = match q.state.as_deref() {
        None | Some("") => None,
        Some(s) => Some(parse_credit_note_state(s)?),
    };
    let filter = CreditNoteFilter {
        invoice_id,
        state: cn_state,
        limit: q.limit,
        offset: q.offset,
    };
    let rows = CreditNoteRepo::new(&state.ctx.db)
        .list(&filter)
        .await
        .map_err(inv_commands::CoreError::from)?;
    Ok(Json(json!({ "credit_notes": rows })))
}

fn parse_credit_note_state(s: &str) -> Result<CreditNoteState, ApiError> {
    match s {
        "draft" => Ok(CreditNoteState::Draft),
        "issued" => Ok(CreditNoteState::Issued),
        other => Err(ApiError::BadRequest(format!(
            "invalid credit-note state `{other}` (want draft | issued)"
        ))),
    }
}
