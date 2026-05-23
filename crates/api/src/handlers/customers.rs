//! Customer routes — create / get / list.
//!
//! `hop-top-inv-commands` does not ship a `customer_create` command (customers
//! are external references, not managed entities — see design §2.3).
//! The API still needs a way to seed them, so this handler bypasses the
//! command layer and writes through [`CustomerRepo`] directly.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};

use hop_top_inv_core::domain::address::Address;
use hop_top_inv_core::domain::customer::Customer;
use hop_top_inv_core::domain::ids::CustomerId;
use hop_top_inv_store::repo::customer::CustomerRepo;

use crate::error::ApiError;
use crate::state::ApiState;

// =============================================================================
// POST /v1/customers
// =============================================================================

/// Body for [`create`].
#[derive(Debug, Deserialize)]
pub struct CreateBody {
    /// Display name (rendered on invoices).
    pub display_name: String,
    /// Email (optional).
    #[serde(default)]
    pub email: Option<String>,
    /// Postal address. Country is required.
    pub address: Address,
    /// Free-form metadata bag.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

/// `POST /v1/customers`.
pub async fn create(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<CreateBody>,
) -> Result<(StatusCode, Json<Customer>), ApiError> {
    if body.address.country.trim().is_empty() {
        return Err(ApiError::BadRequest("address.country is required".into()));
    }
    let now = Utc::now();
    let customer = Customer {
        id: CustomerId::new(),
        display_name: body.display_name,
        email: body.email,
        address: body.address,
        metadata: body.metadata,
        created_at: now,
        updated_at: now,
    };
    CustomerRepo::new(&state.ctx.db)
        .save(&customer)
        .await
        .map_err(hop_top_inv_commands::CoreError::from)?;
    Ok((StatusCode::CREATED, Json(customer)))
}

// =============================================================================
// GET /v1/customers/:id
// =============================================================================

/// `GET /v1/customers/{id}`.
pub async fn get(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<Customer>, ApiError> {
    let id: CustomerId = id
        .parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid customer id: {e}")))?;
    let c = CustomerRepo::new(&state.ctx.db)
        .get(&id)
        .await
        .map_err(hop_top_inv_commands::CoreError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("customer {id}")))?;
    Ok(Json(c))
}

// =============================================================================
// GET /v1/customers
// =============================================================================

/// Query parameters for [`list`].
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    /// Max rows (default 100, max 1000).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Offset for paging.
    #[serde(default)]
    pub offset: Option<i64>,
}

/// `GET /v1/customers`.
pub async fn list(
    State(state): State<Arc<ApiState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);
    let rows = CustomerRepo::new(&state.ctx.db)
        .list(limit, offset)
        .await
        .map_err(hop_top_inv_commands::CoreError::from)?;
    Ok(Json(json!({ "customers": rows })))
}
