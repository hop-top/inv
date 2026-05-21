//! Error type surfaced by the MCP adapter.
//!
//! Wraps `inv_commands::CoreError` and `inv_store::StoreError`, and
//! converts them into rmcp's wire-level [`rmcp::ErrorData`] (a.k.a.
//! `McpError`). Validation failures map to `invalid_params`; FSM /
//! not-found / repo errors map to `internal_error` with the original
//! message attached.

use rmcp::ErrorData as RmcpErrorData;
use thiserror::Error;

use inv_commands::CoreError;
use inv_store::StoreError;

/// Errors surfaced by the MCP adapter layer.
#[derive(Debug, Error)]
pub enum McpError {
    /// Command-layer error.
    #[error("inv command error: {0}")]
    Core(#[from] CoreError),
    /// Store-layer error (rare; commands wrap this already).
    #[error("inv store error: {0}")]
    Store(#[from] StoreError),
    /// JSON decode of a tool's input payload failed.
    #[error("invalid input: {0}")]
    Decode(String),
    /// URI parse failure (e.g. `inv://invoice/<bad-id>`).
    #[error("invalid resource uri: {0}")]
    InvalidUri(String),
    /// Resource not found.
    #[error("resource not found: {0}")]
    NotFound(String),
}

impl McpError {
    /// Convert into rmcp's wire-level error. Validation +
    /// [`CoreError::Validation`] map to `invalid_params`; everything
    /// else maps to `internal_error`. Each carries the underlying
    /// message verbatim so the MCP client sees something useful.
    pub fn to_rmcp(&self) -> RmcpErrorData {
        let msg = self.to_string();
        match self {
            McpError::Decode(_) | McpError::InvalidUri(_) => {
                RmcpErrorData::invalid_params(msg, None)
            }
            McpError::NotFound(_) => RmcpErrorData::invalid_params(msg, None),
            McpError::Core(CoreError::Validation(_))
            | McpError::Core(CoreError::Idempotency(_))
            | McpError::Core(CoreError::NotFound(_))
            | McpError::Core(CoreError::FsmTransition(_)) => {
                RmcpErrorData::invalid_params(msg, None)
            }
            McpError::Core(_) | McpError::Store(_) => {
                RmcpErrorData::internal_error(msg, None)
            }
        }
    }
}

impl From<McpError> for RmcpErrorData {
    fn from(err: McpError) -> Self {
        err.to_rmcp()
    }
}
