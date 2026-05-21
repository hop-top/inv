//! Minimal bearer-token authoriser.
//!
//! Reads `Authorization: Bearer <token>` and matches against the
//! [`ApiConfig::bearer_tokens`](crate::ApiConfig::bearer_tokens) allowlist.
//! If the list is empty (default), authentication is bypassed — useful
//! for local dev and the in-process test harness, never for production.
//!
//! Constant-time comparison (via `subtle`-style folding) is overkill at
//! v1 with a handful of tokens; the loop below uses `==` and accepts the
//! timing-leak risk until we wire a real auth backend (T-0029).
//!
//! Public routes (`/healthz`, `/v/{token}`) are NOT subject to this
//! middleware — see the router assembly in [`crate::router`].

use std::sync::Arc;

use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;

use crate::error::ApiError;
use crate::state::ApiState;

/// Per-request middleware: require a bearer token unless the allowlist
/// is empty.
pub async fn require_bearer(
    State(state): State<Arc<ApiState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, ApiError> {
    if state.config.bearer_tokens.is_empty() {
        return Ok(next.run(req).await);
    }
    let header = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;
    let token = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
        .ok_or(ApiError::Unauthorized)?
        .trim();
    if state.config.bearer_tokens.iter().any(|t| t == token) {
        Ok(next.run(req).await)
    } else {
        Err(ApiError::Unauthorized)
    }
}
