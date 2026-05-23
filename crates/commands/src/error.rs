//! Error type surfaced by every command in this crate.
//!
//! Each variant carries enough context for an adapter (CLI, API, MCP,
//! …) to render a useful error to the caller without inspecting the
//! source chain. Specifically:
//!
//! - [`CoreError::Validation`] — input failed validation. Carries a
//!   single human-readable message.
//! - [`CoreError::Idempotency`] — caller-supplied idempotency key
//!   collided with a record of a different shape. (Same-shape replays
//!   succeed silently; this variant only fires when the caller would
//!   be lied to.)
//! - [`CoreError::FsmTransition`] — the FSM rejected the requested
//!   transition. Wraps the underlying [`TransitionError`].
//! - [`CoreError::Repo`] — the store layer failed. Wraps [`StoreError`].
//! - [`CoreError::Tax`] — the tax resolver rejected an input.
//! - [`CoreError::Render`] — HTML rendering failed.
//! - [`CoreError::Pdf`] — PDF rendering failed.
//! - [`CoreError::NotFound`] — the caller asked us to operate on an
//!   entity that doesn't exist.
//! - [`CoreError::NotImplemented`] — a destination scheme is recognised
//!   but not implemented at v1 (e.g. `bus://`, `webhook://`, `link://`).

use thiserror::Error;

use hop_top_inv_core::render::{PdfError, RenderError};
use hop_top_inv_core::state::TransitionError;
use hop_top_inv_core::tax::TaxError;
use hop_top_inv_store::StoreError;

/// Errors surfaced by every command in `hop-top-inv-commands`.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Input failed validation up front (before the DB or FSM was touched).
    #[error("validation error: {0}")]
    Validation(String),

    /// Idempotency-key collision with an existing record of a different
    /// shape. (Same-shape replays are NOT errors — they return the
    /// original output.)
    #[error("idempotency conflict: {0}")]
    Idempotency(String),

    /// FSM rejected the requested transition.
    #[error("fsm transition error: {0}")]
    FsmTransition(#[from] TransitionError),

    /// Underlying store error (sqlx / mapping / id parse / etc.).
    #[error("repository error: {0}")]
    Repo(#[from] StoreError),

    /// Tax resolver rejected an input.
    #[error("tax error: {0}")]
    Tax(#[from] TaxError),

    /// HTML render failed (template missing, syntax error, etc.).
    #[error("render error: {0}")]
    Render(#[from] RenderError),

    /// PDF render failed (engine missing / failed).
    #[error("pdf render error: {0}")]
    Pdf(#[from] PdfError),

    /// The caller asked us to act on a record that doesn't exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// Destination scheme recognised but not implemented at v1.
    #[error("not implemented: {0}")]
    NotImplemented(String),
}
