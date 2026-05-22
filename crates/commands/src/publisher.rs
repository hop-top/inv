//! [`Publisher`] trait + [`PublishError`] — the canonical bus seam every
//! command consumes.
//!
//! The trait used to live in `inv-bus` (T-0014/T-0027). It moved here so
//! [`crate::CoreCtx`] can hold an `Option<Arc<dyn Publisher>>` without
//! creating an `inv-commands` → `inv-bus` cycle (`inv-bus` already
//! depends on `inv-commands` for the [`crate::EmittedEvent`] envelope and
//! the per-command surface the outbox relay re-derives from).
//!
//! `inv-bus` re-exports both items as `inv_bus::{Publisher, PublishError}`
//! so existing call-sites keep compiling unchanged.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use thiserror::Error;

/// Error returned by [`Publisher::publish`].
#[derive(Debug, Error)]
pub enum PublishError {
    /// Backend (kit bus, log target, in-memory sink, …) refused the publish.
    #[error("publish failed: {0}")]
    Backend(String),

    /// JSON serialisation of the payload failed.
    #[error("payload json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Async trait every bus publisher implements.
///
/// `topic` is the dotted name (e.g. `inv.billing.invoice.issued`),
/// `payload` is the JSON body, `occurred_at` is the wall-clock the event
/// represents (taken from the originating command's clock so the bus
/// timestamp lines up with the audit row).
#[async_trait]
pub trait Publisher: Send + Sync {
    /// Publish `payload` on `topic`. Backends MAY retry internally; an
    /// `Err` return means the publish was not accepted.
    async fn publish(
        &self,
        topic: &str,
        payload: Value,
        occurred_at: DateTime<Utc>,
    ) -> Result<(), PublishError>;
}
