//! Storage-layer error type.

use thiserror::Error;

/// Result alias for store operations.
pub type Result<T> = std::result::Result<T, StoreError>;

/// Storage errors surfaced by repository methods and the pool/migrate
/// helpers.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Underlying sqlx error.
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    /// Migration error.
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// JSON (de)serialisation failed (e.g., `address_json`, metadata).
    #[error("json (de)serialisation error: {0}")]
    Json(#[from] serde_json::Error),
    /// Decimal parse failed reading TEXT-form money column.
    #[error("decimal parse error: {0}")]
    Decimal(#[from] rust_decimal::Error),
    /// Domain id parse failed (typeid prefix mismatch, etc.).
    #[error("id parse error: {0}")]
    Id(String),
    /// Caller passed an unsupported DSN scheme.
    #[error("unsupported DSN scheme: {0}")]
    UnsupportedDsn(String),
    /// Row's column held a value that couldn't be mapped to the domain
    /// enum / typed value (e.g., unknown invoice state string).
    #[error("invalid stored value for `{column}`: {value}")]
    InvalidValue {
        /// Column whose stored value was rejected.
        column: &'static str,
        /// The offending value as a string.
        value: String,
    },
    /// Filesystem / IO failure surfacing through a blob backend.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Blob URI / signed-URL parse / verification failure.
    #[error("blob error: {0}")]
    Blob(String),
    /// Anything else (rare).
    #[error("{0}")]
    Other(String),
}
