//! Helpers shared by every tool module.
//!
//! - [`mcp_actor`] / [`mcp_channel`] — the fixed (actor, channel) every
//!   MCP-originated command call carries.
//! - [`to_value`] — serialize a command output to `serde_json::Value`
//!   and wrap MCP error conversion.
//! - [`success_json`] — turn a serializable value into a
//!   [`rmcp::model::CallToolResult`] with both unstructured text content
//!   and structured_content (per MCP convention; see
//!   `CallToolResult::structured`).

use rmcp::model::CallToolResult;
use serde::Serialize;

use inv_commands::{Actor, Channel};

use crate::error::McpError;

/// Single Actor used for every MCP-originated command call.
pub fn mcp_actor() -> Actor {
    Actor::Mcp {
        name: "mcp".to_string(),
    }
}

/// Single Channel constant.
pub fn mcp_channel() -> Channel {
    Channel::Mcp
}

/// Serialize a value to `serde_json::Value`, wrapping errors as
/// [`McpError::Decode`].
pub fn to_value<T: Serialize>(v: &T) -> Result<serde_json::Value, McpError> {
    serde_json::to_value(v).map_err(|e| McpError::Decode(e.to_string()))
}

/// Convenience: serialize the output and wrap it in a
/// `CallToolResult::structured(...)`. The "text" arm of the result mirrors
/// the structured content (per MCP convention) so clients that don't
/// inspect `structured_content` still see the payload.
pub fn success_json<T: Serialize>(v: &T) -> Result<CallToolResult, McpError> {
    let value = to_value(v)?;
    Ok(CallToolResult::structured(value))
}
