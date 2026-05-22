//! Minimal bearer-token authoriser.
//!
//! Reads `Authorization: Bearer <token>` and matches against the
//! [`ApiConfig::bearer_tokens`](crate::ApiConfig::bearer_tokens) allowlist.
//! If the list is empty (default), authentication is bypassed — useful
//! for local dev and the in-process test harness, never for production.
//!
//! Token comparison goes through [`subtle::ConstantTimeEq`] so that a
//! same-length token that differs only in the trailing bytes does NOT
//! take observably less time to reject than one that differs in the
//! leading bytes. Side-channel risk is low at v1 (no real auth backend
//! yet, alpha-stage; T-0029 lands the real identity layer), but the
//! pattern is cheap and removes a footgun from any future copy-paste.
//!
//! Public routes (`/healthz`, `/v/{token}`) are NOT subject to this
//! middleware — see the router assembly in [`crate::router`].

use std::sync::Arc;

use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use subtle::ConstantTimeEq;

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
    if state
        .config
        .bearer_tokens
        .iter()
        .any(|t| constant_time_eq(t.as_bytes(), token.as_bytes()))
    {
        Ok(next.run(req).await)
    } else {
        Err(ApiError::Unauthorized)
    }
}

/// Constant-time byte-slice equality.
///
/// Returns `false` for different-length inputs without inspecting bytes
/// (the length itself is already public — Content-Length / header size
/// leaks it — so this short-circuit is safe). Equal-length inputs are
/// folded through [`subtle::ConstantTimeEq::ct_eq`] which compares every
/// byte regardless of mismatch position.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;

    #[test]
    fn matching_bytes_eq() {
        assert!(constant_time_eq(b"secret-token", b"secret-token"));
    }

    #[test]
    fn mismatched_bytes_neq() {
        assert!(!constant_time_eq(b"secret-token", b"secret-tokeN"));
        assert!(!constant_time_eq(b"secret-token", b"Xecret-token"));
    }

    #[test]
    fn different_lengths_neq() {
        assert!(!constant_time_eq(b"short", b"shorter"));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    #[test]
    fn empty_eq() {
        assert!(constant_time_eq(b"", b""));
    }
}
