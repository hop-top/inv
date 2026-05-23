//! hop-top-inv-mcp — MCP server channel adapter. Thin shell over
//! `hop-top-inv-commands`.
//!
//! Per design §10, the MCP channel exposes every command as a tool and
//! every readable entity (invoice, credit note, schedule, reminder,
//! customer) as a resource at `inv://<kind>/<id>`. At v1 only the
//! stdio transport is wired (design §11: `mcp_stdio = true`);
//! HTTP/SSE transports land in v1.1.
//!
//! ## Quick start
//!
//! ```no_run
//! use std::sync::Arc;
//! use hop_top_inv_commands::CoreCtx;
//!
//! # async fn run(ctx: Arc<CoreCtx>) -> anyhow::Result<()> {
//! hop_top_inv_mcp::run_stdio(ctx).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Architecture
//!
//! - [`server`] — [`InvMcpServer`] holds `Arc<CoreCtx>` and implements
//!   rmcp's `ServerHandler` via the `#[tool_router]` / `#[tool_handler]`
//!   proc-macros (rmcp 1.7 idiomatic surface).
//! - [`tools`] — one submodule per command grouping. Each tool decodes
//!   a JSON-schema'd input struct (derived via `schemars::JsonSchema`)
//!   into the matching `hop_top_inv_commands` typed input.
//! - [`resources`] — `inv://invoice/<id>` etc. resolvers.
//! - [`error`] — [`McpError`] + conversions to rmcp's wire-level error.

#![deny(missing_docs)]

pub mod error;
pub mod resources;
pub mod server;
pub mod tools;

pub use error::McpError;
pub use server::InvMcpServer;

use std::sync::Arc;

use hop_top_inv_commands::CoreCtx;

/// Boot the MCP server on the stdio transport.
///
/// Reads JSON-RPC messages from stdin and writes responses to stdout.
/// Returns when the peer closes the transport or the service's
/// internal task ends.
pub async fn run_stdio(ctx: Arc<CoreCtx>) -> Result<(), McpError> {
    use rmcp::transport::io::stdio;
    use rmcp::ServiceExt;

    let server = InvMcpServer::new(ctx);
    let transport = stdio();
    let running = server
        .serve(transport)
        .await
        .map_err(|e| McpError::Decode(format!("mcp serve init failed: {e}")))?;
    running
        .waiting()
        .await
        .map_err(|e| McpError::Decode(format!("mcp serve loop ended: {e}")))?;
    Ok(())
}
