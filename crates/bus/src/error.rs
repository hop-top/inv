//! Error types for publish, relay, and inbox ingest.
//!
//! `PublishError` lives in `hop-top-inv-commands` (T-0031) so [`hop_top_inv_commands::CoreCtx`]
//! can hold an `Option<Arc<dyn Publisher>>` without a dep cycle. This crate
//! re-exports it from its root for backwards compatibility.

use thiserror::Error;

pub use hop_top_inv_commands::PublishError;

/// Error returned by [`crate::run_outbox_relay`].
#[derive(Debug, Error)]
pub enum RelayError {
    /// Publishing one of the events failed.
    #[error("publish: {0}")]
    Publish(#[from] PublishError),

    /// Underlying store error (query, mark-published, …).
    #[error("store: {0}")]
    Store(#[from] hop_top_inv_store::StoreError),
}

/// Error returned by [`crate::dispatch_inbound_event`].
#[derive(Debug, Error)]
pub enum IngestError {
    /// Underlying store error (insert, dedup lookup).
    #[error("store: {0}")]
    Store(#[from] hop_top_inv_store::StoreError),
}
