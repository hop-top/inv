//! inv-api — HTTP API channel adapter (axum 0.8).
//!
//! Exposes the [`inv_commands`] operation surface as a REST API per design
//! §10 and ships the supporting plumbing every adapter needs:
//!
//! - RFC 9457 problem-detail responses ([`error::ApiError`]).
//! - Per-request tracing span + permissive CORS (will be tightened later)
//!   + request body size limit ([`middleware`]).
//! - Minimal bearer-token authoriser ([`auth`]). The public signed-link
//!   route at `/v/:token` is exempt.
//! - HMAC-SHA256 signed shareable links for the `link://` delivery
//!   channel ([`signed_link`]).
//! - Outbound webhook signature on `send_invoice` when the destination is
//!   `webhook://...` ([`webhook`]).
//!
//! Channel adapters in this codebase stay deliberately thin: every route
//! parses its body, hands a typed input to the matching command, and maps
//! the result back to JSON. Business logic lives in `inv-commands`.

#![deny(missing_docs)]

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;

pub mod auth;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod signed_link;
pub mod state;
pub mod webhook;

pub use error::ApiError;
pub use state::{ApiConfig, ApiState};

/// Build the assembled router.
///
/// Wires every `/v1/*` resource (with bearer-token auth) plus the public
/// `/v/{token}` view route, `/healthz`, and the read-only `/v1/tax/rates`
/// helper. The caller is responsible for binding it to a `tokio::net`
/// listener — typically the `inv-server` binary at T-0020.
pub fn router(state: Arc<ApiState>) -> Router {
    // Public, unauthenticated routes (signed-link view + health).
    let public = Router::new()
        .route("/healthz", get(handlers::healthz))
        .route("/v/{token}", get(handlers::view::view_invoice));

    // Authenticated v1 routes.
    let v1 = Router::new()
        // Invoices.
        .route(
            "/invoices",
            post(handlers::invoices::draft).get(handlers::invoices::list),
        )
        .route("/invoices/{id}", get(handlers::invoices::get))
        .route("/invoices/{id}/issue", post(handlers::invoices::issue))
        .route("/invoices/{id}/send", post(handlers::invoices::send))
        .route("/invoices/{id}/pay", post(handlers::invoices::pay))
        .route("/invoices/{id}/void", post(handlers::invoices::void))
        // Credit notes (create lives under the parent invoice).
        .route(
            "/invoices/{id}/credit-notes",
            post(handlers::credit_notes::draft),
        )
        .route(
            "/credit-notes/{id}/issue",
            post(handlers::credit_notes::issue),
        )
        .route("/credit-notes/{id}", get(handlers::credit_notes::get))
        .route("/credit-notes", get(handlers::credit_notes::list))
        // Schedules.
        .route(
            "/schedules",
            post(handlers::schedules::create).get(handlers::schedules::list),
        )
        .route("/schedules/{id}", get(handlers::schedules::get))
        .route("/schedules/{id}/pause", post(handlers::schedules::pause))
        .route("/schedules/{id}/cancel", post(handlers::schedules::cancel))
        // Reminders.
        .route(
            "/invoices/{id}/reminders",
            post(handlers::reminders::schedule),
        )
        .route("/reminders/{id}/cancel", post(handlers::reminders::cancel))
        .route("/reminders", get(handlers::reminders::list))
        // Customers.
        .route(
            "/customers",
            post(handlers::customers::create).get(handlers::customers::list),
        )
        .route("/customers/{id}", get(handlers::customers::get))
        // Tickers (driven by inv-server on a timer).
        .route("/tick/schedules", post(handlers::invoices::tick_schedules))
        .route("/tick/reminders", post(handlers::invoices::tick_reminders))
        .route("/tick/overdue", post(handlers::invoices::tick_overdue))
        // Tax dump (read-only debug endpoint).
        .route("/tax/rates", get(handlers::invoices::list_tax_rates))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_bearer,
        ));

    let router = Router::new()
        .merge(public)
        .nest("/v1", v1)
        .with_state(state);
    middleware::apply(router)
}
