//! Local bus + subscriber types used by the WebSocket adapter.
//!
//! ## Why a local trait?
//!
//! The inv-bus crate (T-0014) hasn't shipped a concrete publisher /
//! subscriber API yet. To keep this crate fully self-testable today,
//! we define a minimal `Publisher` trait + a `LocalBus` channel-based
//! fanout that any caller (production or tests) can hand to
//! [`crate::router`]. When inv-bus lands, this module will switch to
//! re-exporting that API and the call sites won't change.
//!
//! ## Topic matching
//!
//! Patterns follow the dotted convention captured in design §11:
//!
//! - `*` matches exactly one segment.
//! - `#` matches zero-or-more trailing segments (and ONLY appears at the end).
//! - Anything else matches itself literally.
//!
//! `inv.billing.invoice.#` matches both `inv.billing.invoice.drafted`
//! and `inv.billing.invoice.payment.received`.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::sync::broadcast;

/// A single bus event that flows through [`Publisher`].
#[derive(Debug, Clone)]
pub struct BusMessage {
    /// Dotted topic name.
    pub topic: String,
    /// Event payload.
    pub payload: Value,
    /// Wall-clock at emit time.
    pub timestamp: DateTime<Utc>,
}

/// Minimal publisher interface the WebSocket adapter consumes.
///
/// Production code wires this to `inv-bus` once that crate ships;
/// tests use [`LocalBus`] which is a thin wrapper over a Tokio
/// `broadcast::Sender`.
#[async_trait]
pub trait Publisher: Send + Sync + 'static {
    /// Publish a message to every subscriber.
    async fn publish(&self, msg: BusMessage);
    /// Open a new subscriber. The returned [`Subscriber`] yields every
    /// subsequent [`BusMessage`].
    fn subscribe(&self) -> Subscriber;
}

/// One open subscription.
///
/// Backed by a `broadcast::Receiver`. The adapter wraps a `Subscriber`
/// in a per-connection task that filters messages against the
/// caller's pattern and forwards matches to the WebSocket sender.
pub struct Subscriber {
    rx: broadcast::Receiver<BusMessage>,
}

impl Subscriber {
    /// Receive the next [`BusMessage`]. Returns `None` if the channel
    /// closed (publisher dropped); a lag error is logged + skipped.
    pub async fn recv(&mut self) -> Option<BusMessage> {
        loop {
            match self.rx.recv().await {
                Ok(m) => return Some(m),
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(lagged = n, "ws subscriber lagged; skipping");
                    continue;
                }
            }
        }
    }
}

/// Channel-based broadcast publisher used until inv-bus ships a real one.
#[derive(Debug, Clone)]
pub struct LocalBus {
    tx: broadcast::Sender<BusMessage>,
}

impl LocalBus {
    /// Construct with a bounded broadcast channel of the given capacity.
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }
}

impl Default for LocalBus {
    fn default() -> Self {
        // 1024 messages is plenty for v1 — slow subscribers will see Lagged
        // and drop the gap (logged in `Subscriber::recv`).
        Self::new(1024)
    }
}

#[async_trait]
impl Publisher for LocalBus {
    async fn publish(&self, msg: BusMessage) {
        // `broadcast::Sender::send` returns Err only when there are no
        // receivers; treat that as a no-op so the producer half of a
        // command flow never panics on a quiescent bus.
        let _ = self.tx.send(msg);
    }

    fn subscribe(&self) -> Subscriber {
        Subscriber {
            rx: self.tx.subscribe(),
        }
    }
}

/// Test if `topic` matches a dotted glob `pattern`. See module docs for
/// the grammar (`*` segment-wild, `#` tail-wild, literals).
pub fn topic_matches(pattern: &str, topic: &str) -> bool {
    let pat_segs: Vec<&str> = pattern.split('.').collect();
    let top_segs: Vec<&str> = topic.split('.').collect();

    let mut i = 0;
    while i < pat_segs.len() {
        // `#` matches the rest of the topic (zero or more segments) — must be terminal.
        if pat_segs[i] == "#" {
            return i == pat_segs.len() - 1;
        }
        if i >= top_segs.len() {
            return false;
        }
        if pat_segs[i] != "*" && pat_segs[i] != top_segs[i] {
            return false;
        }
        i += 1;
    }
    // No `#` consumed the tail; the topic must have exactly as many segments.
    i == top_segs.len()
}

/// Convenience: shared `Arc<dyn Publisher>` for [`crate::router`].
pub type SharedPublisher = Arc<dyn Publisher>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_literal_topic() {
        assert!(topic_matches("inv.billing.invoice.drafted", "inv.billing.invoice.drafted"));
        assert!(!topic_matches("inv.billing.invoice.drafted", "inv.billing.invoice.issued"));
    }

    #[test]
    fn matches_segment_wildcard() {
        assert!(topic_matches("inv.billing.*.drafted", "inv.billing.invoice.drafted"));
        assert!(!topic_matches("inv.billing.*.drafted", "inv.billing.invoice.issued"));
        // `*` matches exactly one segment, not zero.
        assert!(!topic_matches("inv.billing.*.drafted", "inv.billing.drafted"));
    }

    #[test]
    fn matches_tail_wildcard() {
        assert!(topic_matches("inv.billing.invoice.#", "inv.billing.invoice.drafted"));
        assert!(topic_matches(
            "inv.billing.invoice.#",
            "inv.billing.invoice.payment.received"
        ));
        // `#` at the end matches zero-or-more segments — including zero.
        assert!(topic_matches("inv.billing.invoice.#", "inv.billing.invoice"));
    }

    #[tokio::test]
    async fn local_bus_fans_out() {
        let bus = LocalBus::new(8);
        let mut sub = bus.subscribe();

        bus.publish(BusMessage {
            topic: "inv.billing.invoice.drafted".into(),
            payload: serde_json::json!({"x": 1}),
            timestamp: Utc::now(),
        })
        .await;

        let msg = sub.recv().await.expect("recv");
        assert_eq!(msg.topic, "inv.billing.invoice.drafted");
        assert_eq!(msg.payload["x"], 1);
    }
}
