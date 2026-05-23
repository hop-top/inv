//! In-process broadcast publisher.
//!
//! [`BroadcastPublisher`] is a [`Publisher`] backed by a Tokio
//! `broadcast::Sender<BusMessage>`. Every call to [`Publisher::publish`]
//! fans the message out to every live [`BroadcastSubscriber`]. Used by
//! the WebSocket adapter (`hop-top-inv-ws`) so per-connection forwarders can
//! pull the same event stream the outbox relay emits.
//!
//! Unlike [`crate::LoggingPublisher`] / [`crate::InMemoryPublisher`],
//! this backend exposes a fanout side via [`BroadcastPublisher::subscribe`].
//! It implements the [`Publisher`] trait too, so the outbox relay can
//! drive it identically to any other publisher.
//!
//! Subscribers that fall behind the broadcast channel will see their
//! [`BroadcastSubscriber::recv`] skip the dropped messages and log a
//! `tracing::warn!` with the lag count. Backpressure is intentionally
//! not modeled — slow subscribers should consume the outbox directly.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::error::PublishError;
use crate::publisher::Publisher;

/// A single bus event delivered to [`BroadcastSubscriber`] receivers.
#[derive(Debug, Clone)]
pub struct BusMessage {
    /// Dotted topic name (e.g. `inv.billing.invoice.drafted`).
    pub topic: String,
    /// JSON payload as emitted by the publisher.
    pub payload: Value,
    /// Wall-clock the event represents (taken from the originating
    /// command's clock so this lines up with the audit row).
    pub timestamp: DateTime<Utc>,
}

/// In-process broadcast publisher. Implements [`Publisher`] (so the
/// outbox relay can drive it) and exposes [`Self::subscribe`] for
/// fanout consumers like the WebSocket adapter.
#[derive(Debug, Clone)]
pub struct BroadcastPublisher {
    tx: broadcast::Sender<BusMessage>,
}

impl BroadcastPublisher {
    /// Construct with a bounded broadcast channel of `capacity` slots.
    /// Slow subscribers that lag past `capacity` will see dropped
    /// messages reported through [`BroadcastSubscriber::recv`].
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Open a new subscription. Each subscriber sees every subsequent
    /// [`BusMessage`] published after the call returns.
    pub fn subscribe(&self) -> BroadcastSubscriber {
        BroadcastSubscriber {
            rx: self.tx.subscribe(),
        }
    }
}

impl Default for BroadcastPublisher {
    fn default() -> Self {
        // 1024 messages is plenty for v1 — slow subscribers will see
        // Lagged and drop the gap (logged in `BroadcastSubscriber::recv`).
        Self::new(1024)
    }
}

#[async_trait]
impl Publisher for BroadcastPublisher {
    async fn publish(
        &self,
        topic: &str,
        payload: Value,
        occurred_at: DateTime<Utc>,
    ) -> Result<(), PublishError> {
        // `broadcast::Sender::send` returns Err only when there are no
        // receivers; treat that as a no-op so the outbox relay never
        // fails on a quiescent bus.
        let _ = self.tx.send(BusMessage {
            topic: topic.to_string(),
            payload,
            timestamp: occurred_at,
        });
        Ok(())
    }
}

/// One open subscription.
///
/// Backed by a `broadcast::Receiver`. Callers wrap this in a
/// per-consumer task that filters / forwards each [`BusMessage`].
pub struct BroadcastSubscriber {
    rx: broadcast::Receiver<BusMessage>,
}

impl BroadcastSubscriber {
    /// Receive the next [`BusMessage`]. Returns `None` when the
    /// channel closes (publisher dropped); a lag error is logged
    /// + skipped so the receiver stays live across slow windows.
    pub async fn recv(&mut self) -> Option<BusMessage> {
        loop {
            match self.rx.recv().await {
                Ok(m) => return Some(m),
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(lagged = n, "broadcast subscriber lagged; skipping");
                    continue;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn broadcast_fans_out_to_live_subscribers() {
        let pub_ = BroadcastPublisher::new(8);
        let mut sub = pub_.subscribe();

        pub_.publish("inv.billing.invoice.drafted", json!({"x": 1}), Utc::now())
            .await
            .unwrap();

        let msg = sub.recv().await.expect("recv");
        assert_eq!(msg.topic, "inv.billing.invoice.drafted");
        assert_eq!(msg.payload["x"], 1);
    }

    #[tokio::test]
    async fn publish_with_no_subscribers_is_noop() {
        let pub_ = BroadcastPublisher::new(8);
        pub_.publish("inv.billing.invoice.drafted", json!({}), Utc::now())
            .await
            .unwrap();
    }
}
