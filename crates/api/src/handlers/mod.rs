//! HTTP handlers — one module per resource group.
//!
//! Each module hosts the routes that share a URL prefix in the design's
//! channel-surface table (§10). The actual business logic stays in
//! `hop-top-inv-commands`; these functions only translate JSON ↔ command IO and
//! map errors to [`crate::ApiError`].

pub mod credit_notes;
pub mod customers;
pub mod invoices;
pub mod reminders;
pub mod schedules;
pub mod view;

use axum::http::StatusCode;

/// `GET /healthz` — liveness probe.
///
/// Returns `200 OK` with the literal body "ok" so a `curl` smoke test
/// has something to assert on without parsing JSON.
pub async fn healthz() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}
