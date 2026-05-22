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
//! and the per-connection forwarder pushes any matching
//! [`inv_bus::BusMessage`] at it. The publisher type is
//! [`inv_bus::BroadcastPublisher`] — a `Publisher` impl that also
//! exposes a fanout `subscribe()`, so the same publisher the outbox
//! relay drives feeds every live WebSocket subscription.
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

pub mod frames;
pub mod handler;
pub mod ops;
pub mod topics;

use std::sync::Arc;

use axum::routing::get;
use axum::Router;

use inv_commands::CoreCtx;

pub use frames::{
    ClientFrame, ErrorBody, EventFrame, RequestFrame, ResponseBody, ResponseFrame, ServerFrame,
    SubMgmtFrame, SubMgmtOp, SubMgmtPayload,
};
pub use handler::{AppState, SharedPublisher};
pub use ops::{OpError, OP_NAMES};
pub use topics::topic_matches;

// Re-export the bus types the WS adapter exposes on its API surface,
// so callers (and integration tests) don't need a direct `inv-bus`
// dep just to construct the publisher we accept.
pub use inv_bus::{BroadcastPublisher, BroadcastSubscriber, BusMessage};

/// Build the WebSocket router fragment.
///
/// The returned [`Router`] is stateless from axum's perspective — the
/// [`CoreCtx`] + the broadcast publisher live inside an [`AppState`]
/// passed via `with_state`. The caller (typically inv-api at T-0017)
/// merges this router onto its top-level one with
/// [`axum::Router::merge`].
pub fn router(ctx: Arc<CoreCtx>, publisher: SharedPublisher) -> Router {
    Router::new()
        .route("/ws", get(handler::ws_upgrade))
        .with_state(AppState { ctx, publisher })
}
