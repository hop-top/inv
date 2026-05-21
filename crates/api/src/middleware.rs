//! Per-request middleware: tracing span + CORS + body size limit.
//!
//! Applied at the top of the router via [`apply`]. We deliberately keep
//! the stack small at v1 — bearer auth is wired separately on the
//! `/v1/*` subtree only.

use axum::extract::DefaultBodyLimit;
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// Default body limit: 1 MiB. Tightened later; large enough for any
/// realistic invoice POST payload.
pub const DEFAULT_BODY_LIMIT_BYTES: usize = 1024 * 1024;

/// Wrap `router` with the default middleware stack.
///
/// Order (outermost-first):
///
/// 1. `tracing` — emit a span per request (method, URI, status).
/// 2. `CORS` — permissive at v1 (every origin, every method). Tightened
///    when the operator-facing dashboard lands.
/// 3. `DefaultBodyLimit` — reject bodies larger than
///    [`DEFAULT_BODY_LIMIT_BYTES`] before they hit a handler.
///
/// We pin the body limit via axum's native [`DefaultBodyLimit`] rather
/// than `tower-http::RequestBodyLimitLayer` because the latter wraps the
/// body in a type that doesn't implement `Default`, which collides with
/// axum's `Layer` requirements on the assembled router.
pub fn apply<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router
        .layer(DefaultBodyLimit::max(DEFAULT_BODY_LIMIT_BYTES))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
}
