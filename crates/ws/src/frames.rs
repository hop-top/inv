//! JSON frame protocol for the inv WebSocket adapter.
//!
//! ## Wire shapes
//!
//! Request (client → server):
//!
//! ```jsonc
//! { "id": "1", "op": "invoice.draft", "payload": { ... } }
//! ```
//!
//! Subscribe (client → server) — a special op shape carrying a topic pattern:
//!
//! ```jsonc
//! { "id": "2", "op": "subscribe",   "payload": { "pattern": "inv.billing.invoice.#" } }
//! { "id": "3", "op": "unsubscribe", "payload": { "sub_id": "sub_1" } }
//! ```
//!
//! Response (server → client):
//!
//! ```jsonc
//! { "id": "1", "result": { ... } }
//! { "id": "1", "error":  { "code": "validation", "message": "..." } }
//! ```
//!
//! Event (server → client; pushed for matching subscriptions):
//!
//! ```jsonc
//! { "topic": "inv.billing.invoice.issued", "payload": { ... }, "timestamp": "2026-..." }
//! ```
//!
//! ## Decoding
//!
//! [`ClientFrame`] is the inbound enum; we untag it by inspecting the
//! `op` field rather than using `#[serde(tag = ...)]` because the
//! request frame already carries `op` as a free-form string (any
//! command name in [`crate::ops`]) and we want the `subscribe` /
//! `unsubscribe` ops to deserialize their payload into a typed shape.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One frame received from a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ClientFrame {
    /// A subscribe / unsubscribe envelope — recognised first because the
    /// `op` discriminator is fixed and the `payload` shape is typed.
    SubMgmt(SubMgmtFrame),
    /// Anything else — the dispatcher in [`crate::ops`] decides what to
    /// do based on `op`.
    Request(RequestFrame),
}

/// `{ id, op, payload }` — a generic request frame.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestFrame {
    /// Correlation id; copied verbatim into the response frame.
    pub id: String,
    /// Operation name (e.g. `"invoice.draft"`). See [`crate::ops::dispatch`].
    pub op: String,
    /// Operation payload. Decoded into the target Input type by the dispatcher.
    #[serde(default)]
    pub payload: Value,
}

/// Subscribe / unsubscribe envelope. Matches `op == "subscribe"` or
/// `op == "unsubscribe"` only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubMgmtFrame {
    /// Correlation id.
    pub id: String,
    /// Always either `"subscribe"` or `"unsubscribe"`.
    pub op: SubMgmtOp,
    /// Typed payload.
    pub payload: SubMgmtPayload,
}

/// Two kinds of subscription management op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubMgmtOp {
    /// Open a new subscription.
    Subscribe,
    /// Cancel an existing subscription.
    Unsubscribe,
}

/// Payload for a subscribe / unsubscribe op.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SubMgmtPayload {
    /// `subscribe` shape — caller-supplied topic pattern.
    Subscribe {
        /// Topic pattern (`*` matches one segment, `#` matches the rest).
        pattern: String,
    },
    /// `unsubscribe` shape — caller-supplied subscription id (returned
    /// in the prior subscribe response's `result.sub_id`).
    Unsubscribe {
        /// Subscription id to cancel.
        sub_id: String,
    },
}

/// One frame sent by the server.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ServerFrame {
    /// Response to a [`RequestFrame`] or [`SubMgmtFrame`].
    Response(ResponseFrame),
    /// Server-pushed bus event.
    Event(EventFrame),
}

/// `{ id, result }` or `{ id, error }`.
#[derive(Debug, Clone, Serialize)]
pub struct ResponseFrame {
    /// Correlation id (copied from the request).
    pub id: String,
    /// Either the success payload or an error envelope.
    #[serde(flatten)]
    pub body: ResponseBody,
}

/// Discriminator between success and error.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ResponseBody {
    /// Success.
    Result {
        /// Operation-specific result JSON.
        result: Value,
    },
    /// Error.
    Error {
        /// Error envelope.
        error: ErrorBody,
    },
}

/// `{ code, message }` — error body for the response frame.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    /// Short tag (e.g. `"validation"`, `"unknown_op"`, `"internal"`).
    pub code: String,
    /// Human-readable description.
    pub message: String,
}

/// `{ topic, payload, timestamp }` — server-pushed bus event.
#[derive(Debug, Clone, Serialize)]
pub struct EventFrame {
    /// Dotted topic name.
    pub topic: String,
    /// Event payload (verbatim from the publisher).
    pub payload: Value,
    /// Wall-clock at emit time.
    pub timestamp: DateTime<Utc>,
}

impl ResponseFrame {
    /// Build a success response frame.
    pub fn result(id: impl Into<String>, result: Value) -> Self {
        Self {
            id: id.into(),
            body: ResponseBody::Result { result },
        }
    }

    /// Build an error response frame.
    pub fn error(id: impl Into<String>, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            body: ResponseBody::Error {
                error: ErrorBody {
                    code: code.into(),
                    message: message.into(),
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_frame_round_trips() {
        let raw = r#"{"id":"1","op":"invoice.draft","payload":{"customer_id":"customer_xyz"}}"#;
        let frame: ClientFrame = serde_json::from_str(raw).unwrap();
        match frame {
            ClientFrame::Request(r) => {
                assert_eq!(r.id, "1");
                assert_eq!(r.op, "invoice.draft");
                assert_eq!(r.payload["customer_id"], "customer_xyz");
            }
            _ => panic!("expected Request, got {frame:?}"),
        }
    }

    #[test]
    fn subscribe_frame_decodes_typed() {
        let raw = r#"{"id":"2","op":"subscribe","payload":{"pattern":"inv.billing.invoice.#"}}"#;
        let frame: ClientFrame = serde_json::from_str(raw).unwrap();
        match frame {
            ClientFrame::SubMgmt(SubMgmtFrame {
                id,
                op: SubMgmtOp::Subscribe,
                payload: SubMgmtPayload::Subscribe { pattern },
            }) => {
                assert_eq!(id, "2");
                assert_eq!(pattern, "inv.billing.invoice.#");
            }
            other => panic!("expected SubMgmt subscribe, got {other:?}"),
        }
    }

    #[test]
    fn unsubscribe_frame_decodes_typed() {
        let raw = r#"{"id":"3","op":"unsubscribe","payload":{"sub_id":"sub_1"}}"#;
        let frame: ClientFrame = serde_json::from_str(raw).unwrap();
        match frame {
            ClientFrame::SubMgmt(SubMgmtFrame {
                id,
                op: SubMgmtOp::Unsubscribe,
                payload: SubMgmtPayload::Unsubscribe { sub_id },
            }) => {
                assert_eq!(id, "3");
                assert_eq!(sub_id, "sub_1");
            }
            other => panic!("expected SubMgmt unsubscribe, got {other:?}"),
        }
    }

    #[test]
    fn success_response_serializes() {
        let f = ResponseFrame::result("1", serde_json::json!({"ok": true}));
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""id":"1""#));
        assert!(json.contains(r#""result":{"ok":true}"#));
    }

    #[test]
    fn error_response_serializes() {
        let f = ResponseFrame::error("1", "validation", "bad input");
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""id":"1""#));
        assert!(json.contains(r#""error":{"code":"validation","message":"bad input"}"#));
    }

    #[test]
    fn event_frame_serializes() {
        let f = EventFrame {
            topic: "inv.billing.invoice.drafted".into(),
            payload: serde_json::json!({"invoice_id": "invoice_x"}),
            timestamp: chrono::Utc::now(),
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""topic":"inv.billing.invoice.drafted""#));
    }
}
