//! Error types for publish, relay, and inbox ingest.

use thiserror::Error;

/// Error returned by [`crate::Publisher::publish`].
#[derive(Debug, Error)]
pub enum PublishError {
    /// Backend (kit bus, log target, in-memory sink, …) refused the publish.
    #[error("publish failed: {0}")]
    Backend(String),

    /// JSON serialisation of the payload failed.
    #[error("payload json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Error returned by [`crate::run_outbox_relay`].
#[derive(Debug, Error)]
pub enum RelayError {
    /// Publishing one of the events failed.
    #[error("publish: {0}")]
    Publish(#[from] PublishError),

    /// Underlying store error (query, mark-published, …).
    #[error("store: {0}")]
    Store(#[from] inv_store::StoreError),

    /// Sqlx error from a raw query path (credit-note pending-outbox scan
    /// while T-0024's symmetric API lands).
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
}

/// Error returned by [`crate::dispatch_inbound_event`].
#[derive(Debug, Error)]
pub enum IngestError {
    /// Underlying store error (insert, dedup lookup).
    #[error("store: {0}")]
    Store(#[from] inv_store::StoreError),
}
