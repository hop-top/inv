//! inv-ws — WebSocket channel adapter built on axum's `extract::ws`.
//!
//! Ships an [`axum::Router`] fragment that exposes `GET /ws`. The shared
//! HTTP+WS server lives in `inv-api` (T-0017); this crate is intended
//! to be merged onto its router via [`axum::Router::merge`].
//!
//! ## Frame protocol
//!
//! All frames are JSON text. See [`frames`] for the full grammar; the
//! short version:
//!
//! - Client → server: `{ "id", "op", "payload" }`. `op` is one of the
//!   names listed in [`ops::OP_NAMES`] (plus `"subscribe"` /
//!   `"unsubscribe"` for bus topic management).
//! - Server → client: `{ "id", "result" }` on success or `{ "id",
//!   "error": { "code", "message" } }` on failure.
//! - Server-pushed events: `{ "topic", "payload", "timestamp" }`.
//!
//! ## Bus integration
//!
//! Subscribers register a topic pattern (`inv.billing.invoice.#`, etc.)
//! and the per-connection forwarder pushes any matching [`bus::BusMessage`]
//! at it. The publisher trait is local to this crate until inv-bus
//! (T-0014) ships a real one — see [`bus`] for the rationale.
//!
//! ## Topology
//!
//! ```text
//!   GET /ws
//!     │
//!     ▼
//!   ws_upgrade ── State<AppState{ctx, publisher}>
//!     │
//!     ▼
//!   per-connection task ─┬─► read loop  ─► ops::dispatch ─► inv-commands
//!                        │                          │
//!                        │                          └─► CoreCtx (DB/tax/blob)
//!                        │
//!                        └─► writer task  ◄─ mpsc<ServerFrame>  ◄─ sub-fwd tasks
//! ```

#![deny(missing_docs)]

pub mod bus;
pub mod frames;
pub mod handler;
pub mod ops;

use std::sync::Arc;

use axum::routing::get;
use axum::Router;

use inv_commands::CoreCtx;

pub use bus::{BusMessage, LocalBus, Publisher, SharedPublisher, Subscriber};
pub use frames::{
    ClientFrame, ErrorBody, EventFrame, RequestFrame, ResponseBody, ResponseFrame, ServerFrame,
    SubMgmtFrame, SubMgmtOp, SubMgmtPayload,
};
pub use handler::AppState;
pub use ops::{OpError, OP_NAMES};

/// Build the WebSocket router fragment.
///
/// The returned [`Router`] is stateless from axum's perspective — the
/// [`CoreCtx`] + [`Publisher`] live inside an [`AppState`] passed via
/// `with_state`. The caller (typically inv-api at T-0017) merges this
/// router onto its top-level one with [`axum::Router::merge`].
pub fn router(ctx: Arc<CoreCtx>, publisher: SharedPublisher) -> Router {
    Router::new()
        .route("/ws", get(handler::ws_upgrade))
        .with_state(AppState { ctx, publisher })
}
