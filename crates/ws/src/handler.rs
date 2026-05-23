//! WebSocket upgrade handler + per-connection task.
//!
//! On every connection we spawn a single tokio task that:
//!
//! 1. Splits the [`axum::extract::ws::WebSocket`] into sender + receiver.
//! 2. Reads each inbound frame, decodes it as a [`ClientFrame`], and
//!    dispatches it (request → ops, subscribe → bus).
//! 3. For subscriptions, spawns a small forwarder task that pulls
//!    [`BusMessage`]s off the bus and writes [`EventFrame`]s to the
//!    sender.
//!
//! All outbound writes go through a `mpsc::Sender<ServerFrame>` so the
//! main read loop and any number of subscription forwarders can share
//! the WebSocket sender without locking.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

use inv_bus::{BroadcastPublisher, BusMessage};
use inv_commands::CoreCtx;

use crate::frames::{
    ClientFrame, EventFrame, RequestFrame, ResponseFrame, ServerFrame, SubMgmtFrame, SubMgmtOp,
    SubMgmtPayload,
};
use crate::ops;
use crate::topics::topic_matches;

/// Convenience alias for the broadcast publisher the WS adapter holds.
/// Concrete (not `dyn`) because we need both `Publisher::publish` and
/// the non-trait `subscribe()` fanout side.
pub type SharedPublisher = Arc<BroadcastPublisher>;

/// Shared state handed to the upgrade handler via `axum::extract::State`.
#[derive(Clone)]
pub struct AppState {
    /// Command context (DB, tax tables, etc.).
    pub ctx: Arc<CoreCtx>,
    /// Broadcast publisher every connection subscribes against.
    pub publisher: SharedPublisher,
}

/// Axum upgrade route. Wired by [`crate::router`] at `GET /ws`.
pub async fn ws_upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| run_connection(socket, state))
}

/// Per-connection driver.
///
/// Owns the lifetime of one client session: spawns a single writer
/// task, drives the read loop until the client closes, and tears
/// every subscription down on exit.
async fn run_connection(socket: WebSocket, state: AppState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Outbound channel — every produced ServerFrame lands here. A
    // background task drains it onto the underlying WebSocket sender.
    let (out_tx, mut out_rx) = mpsc::channel::<ServerFrame>(64);

    // Writer task. Ends when `out_tx` closes (all senders dropped).
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            let json = match serde_json::to_string(&frame) {
                Ok(s) => s,
                Err(e) => {
                    warn!(error = %e, "ws: failed to serialise outbound frame");
                    continue;
                }
            };
            if let Err(e) = ws_tx.send(Message::Text(json.into())).await {
                debug!(error = %e, "ws: send failed; closing writer");
                break;
            }
        }
    });

    // Subscription registry. Map sub_id → cancellation oneshot.
    let mut subs: HashMap<String, oneshot::Sender<()>> = HashMap::new();
    let mut next_sub_id: u64 = 1;

    while let Some(msg) = ws_rx.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                debug!(error = %e, "ws: recv error; closing");
                break;
            }
        };

        match msg {
            Message::Text(text) => {
                handle_text_frame(&state, &out_tx, &mut subs, &mut next_sub_id, text.as_str())
                    .await;
            }
            Message::Binary(_) => {
                let _ = out_tx
                    .send(ServerFrame::Response(ResponseFrame::error(
                        "",
                        "validation",
                        "binary frames are not supported; send JSON text frames",
                    )))
                    .await;
            }
            Message::Ping(p) => {
                // Axum auto-replies to Ping/Pong on the underlying socket;
                // we don't need to do anything but log.
                debug!(bytes = p.len(), "ws: client ping");
            }
            Message::Pong(_) => {}
            Message::Close(_) => break,
        }
    }

    // Connection closed — drop every sub cancellation handle so the
    // forwarders exit cleanly.
    subs.clear();
    drop(out_tx);
    let _ = writer.await;
}

/// Handle one inbound text frame.
async fn handle_text_frame(
    state: &AppState,
    out_tx: &mpsc::Sender<ServerFrame>,
    subs: &mut HashMap<String, oneshot::Sender<()>>,
    next_sub_id: &mut u64,
    text: &str,
) {
    let frame: ClientFrame = match serde_json::from_str(text) {
        Ok(f) => f,
        Err(e) => {
            let _ = out_tx
                .send(ServerFrame::Response(ResponseFrame::error(
                    "",
                    "validation",
                    format!("frame decode: {e}"),
                )))
                .await;
            return;
        }
    };

    match frame {
        ClientFrame::Request(req) => handle_request(state, out_tx, req).await,
        ClientFrame::SubMgmt(sub) => handle_sub_mgmt(state, out_tx, subs, next_sub_id, sub).await,
    }
}

async fn handle_request(state: &AppState, out_tx: &mpsc::Sender<ServerFrame>, req: RequestFrame) {
    let frame = match ops::dispatch(&state.ctx, &req.op, req.payload, "anonymous").await {
        Ok(result) => ResponseFrame::result(req.id, result),
        Err(err) => {
            let body = err.into_error_body();
            ResponseFrame::error(req.id, body.code, body.message)
        }
    };
    let _ = out_tx.send(ServerFrame::Response(frame)).await;
}

async fn handle_sub_mgmt(
    state: &AppState,
    out_tx: &mpsc::Sender<ServerFrame>,
    subs: &mut HashMap<String, oneshot::Sender<()>>,
    next_sub_id: &mut u64,
    sub: SubMgmtFrame,
) {
    match (sub.op, sub.payload) {
        (SubMgmtOp::Subscribe, SubMgmtPayload::Subscribe { pattern }) => {
            let sub_id = format!("sub_{}", *next_sub_id);
            *next_sub_id += 1;

            let (cancel_tx, mut cancel_rx) = oneshot::channel::<()>();
            let mut subscriber = state.publisher.subscribe();
            let forward_tx = out_tx.clone();
            let pat = pattern.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = &mut cancel_rx => break,
                        m = subscriber.recv() => match m {
                            None => break,
                            Some(BusMessage { topic, payload, timestamp }) => {
                                if !topic_matches(&pat, &topic) {
                                    continue;
                                }
                                let frame = ServerFrame::Event(EventFrame {
                                    topic,
                                    payload,
                                    timestamp,
                                });
                                if forward_tx.send(frame).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            });

            subs.insert(sub_id.clone(), cancel_tx);
            let _ = out_tx
                .send(ServerFrame::Response(ResponseFrame::result(
                    sub.id,
                    serde_json::json!({"sub_id": sub_id, "pattern": pattern}),
                )))
                .await;
        }
        (SubMgmtOp::Unsubscribe, SubMgmtPayload::Unsubscribe { sub_id }) => {
            let removed = subs.remove(&sub_id).is_some();
            let frame = if removed {
                ResponseFrame::result(
                    sub.id,
                    serde_json::json!({"sub_id": sub_id, "cancelled": true}),
                )
            } else {
                ResponseFrame::error(sub.id, "not_found", format!("unknown sub_id: {sub_id}"))
            };
            let _ = out_tx.send(ServerFrame::Response(frame)).await;
        }
        (SubMgmtOp::Subscribe, SubMgmtPayload::Unsubscribe { .. })
        | (SubMgmtOp::Unsubscribe, SubMgmtPayload::Subscribe { .. }) => {
            let _ = out_tx
                .send(ServerFrame::Response(ResponseFrame::error(
                    sub.id,
                    "validation",
                    "sub-mgmt op and payload shape disagree",
                )))
                .await;
        }
    }
}
