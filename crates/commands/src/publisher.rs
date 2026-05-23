//! [`Publisher`] trait + [`PublishError`] — the canonical bus seam every
//! command consumes.
//!
//! The trait used to live in `hop-top-inv-bus` (T-0014/T-0027). It moved here so
//! [`crate::CoreCtx`] can hold an `Option<Arc<dyn Publisher>>` without
//! creating an `hop-top-inv-commands` → `hop-top-inv-bus` cycle (`hop-top-inv-bus` already
//! depends on `hop-top-inv-commands` for the per-command surface the outbox
//! relay re-derives from).
//!
//! `hop-top-inv-bus` re-exports both items as `hop_top_inv_bus::{Publisher, PublishError}`
//! so existing call-sites keep compiling unchanged.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use thiserror::Error;

use crate::ctx::CoreCtx;

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

/// Best-effort synchronous publish from a command body.
///
/// Mirrors T-0031's `api/src/handlers/view.rs` pattern: when
/// `ctx.publisher` is wired the event fires immediately; when absent the
/// call is a no-op. Publish errors are swallowed with a `tracing::warn`
/// because the canonical record is the history-table outbox row — the
/// relay (`crates/bus/src/outbox.rs`) will retry on its next tick.
///
/// Used by every mutating command in this crate that used to build an
/// `EmittedEvent` list. The outbox-relay path remains untouched.
pub(crate) async fn try_publish(
    ctx: &CoreCtx,
    topic: &str,
    payload: Value,
    occurred_at: DateTime<Utc>,
) {
    let Some(publisher) = ctx.publisher.as_ref() else {
        return;
    };
    if let Err(e) = publisher.publish(topic, payload, occurred_at).await {
        tracing::warn!(
            error = %e,
            topic = topic,
            "synchronous publish failed; outbox relay will retry"
        );
    }
}
