//! MCP tools — one per `hop-top-inv-commands` operation.
//!
//! Each submodule wraps a command-layer function as an MCP tool with a
//! JSON-schema'd input and a `serde_json::Value` output. The
//! [`InvMcpServer`][crate::server::InvMcpServer] aggregates them all in
//! a single rmcp `tool_router`.
//!
//! ## Wire-types vs typed-inputs
//!
//! Most commands accept typed inputs with newtype IDs, enums, and
//! `rust_decimal::Decimal`. The MCP wire-form deserializes IDs +
//! decimals from strings (Decimal via the workspace `serde-with-str`
//! feature) and enums via their existing serde reps, so the wire shape
//! is "JSON-friendly" without us needing to re-encode them.
//!
//! Two cross-cutting fields are always supplied by the MCP adapter:
//!
//! - `actor: Actor::Mcp { name: <peer-id-or-"mcp"> }` (the MCP client
//!   does not carry a strongly typed principal at v1 — we record the
//!   constant "mcp" agent name; later tasks can plumb identity).
//! - `channel: Channel::Mcp`.
//!
//! Both are injected by the wrapper at the boundary; callers don't have
//! to (and shouldn't) supply them.

pub mod common;
pub mod credit_notes;
pub mod invoices;
pub mod reminders;
pub mod schedules;
pub mod tickers;

pub use common::*;
