//! [`Publisher`] trait (re-exported from `inv-commands` since T-0031) +
//! the two concrete impls that ship at v1 (kit-backed publisher lands
//! when `poly-kit#sdk-rs-runtime` does).
//!
//! - [`LoggingPublisher`]: writes each publish to `tracing::info!`. Used
//!   on inv-server boot until subscribers come online.
//! - [`InMemoryPublisher`]: collects every publish into a `Vec` for the
//!   test harness.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::sync::Mutex;
use tracing::info;

use crate::error::PublishError;

// The canonical `Publisher` trait lives in `inv-commands` so
// `CoreCtx.publisher` can reference it without a dep cycle (this crate
// already depends on `inv-commands`). Re-exporting keeps every existing
// `inv_bus::Publisher` import compiling unchanged.
pub use inv_commands::Publisher;

// =============================================================================
// LoggingPublisher
// =============================================================================

/// Publisher that writes events to `tracing::info!`. Default for
/// `inv-server` boot before a real bus backend is wired.
///
/// Never fails. Subscribers that need replay semantics should consume
/// the outbox directly.
#[derive(Debug, Default, Clone, Copy)]
pub struct LoggingPublisher;

impl LoggingPublisher {
    /// Construct.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Publisher for LoggingPublisher {
    async fn publish(
        &self,
        topic: &str,
        payload: Value,
        occurred_at: DateTime<Utc>,
    ) -> Result<(), PublishError> {
        info!(
            target: "inv_bus::publish",
            topic = topic,
            occurred_at = %occurred_at,
            payload = %payload,
            "bus publish"
        );
        Ok(())
    }
}

// =============================================================================
// InMemoryPublisher
// =============================================================================

/// One captured event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedEvent {
    /// Topic name.
    pub topic: String,
    /// Payload (JSON).
    pub payload: Value,
    /// Wall-clock the event represents.
    pub occurred_at: DateTime<Utc>,
}

/// Publisher that pushes every event into an internal `Vec`. Tests
/// snapshot the captured events with [`Self::captured`].
///
/// Thread-safe (interior `Mutex`); `&self` so it can be shared as
/// `Arc<dyn Publisher>` and still observed from the test driver.
#[derive(Debug, Default)]
pub struct InMemoryPublisher {
    inner: Mutex<Vec<CapturedEvent>>,
}

impl InMemoryPublisher {
    /// Construct an empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot every event captured so far (clones).
    pub fn captured(&self) -> Vec<CapturedEvent> {
        self.inner.lock().expect("mutex poisoned").clone()
    }

    /// Count captured events.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("mutex poisoned").len()
    }

    /// `true` if no events captured.
    pub fn is_empty(&self) -> bool {
        self.inner.lock().expect("mutex poisoned").is_empty()
    }

    /// Topics of every captured event, in order.
    pub fn topics(&self) -> Vec<String> {
        self.inner
            .lock()
            .expect("mutex poisoned")
            .iter()
            .map(|e| e.topic.clone())
            .collect()
    }
}

#[async_trait]
impl Publisher for InMemoryPublisher {
    async fn publish(
        &self,
        topic: &str,
        payload: Value,
        occurred_at: DateTime<Utc>,
    ) -> Result<(), PublishError> {
        self.inner
            .lock()
            .expect("mutex poisoned")
            .push(CapturedEvent {
                topic: topic.to_string(),
                payload,
                occurred_at,
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn logging_publisher_accepts() {
        let p = LoggingPublisher::new();
        p.publish("inv.billing.invoice.drafted", json!({}), Utc::now())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn in_memory_captures_in_order() {
        let p = InMemoryPublisher::new();
        let t = Utc::now();
        p.publish("a", json!({"n": 1}), t).await.unwrap();
        p.publish("b", json!({"n": 2}), t).await.unwrap();
        let got = p.captured();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].topic, "a");
        assert_eq!(got[1].topic, "b");
        assert_eq!(p.topics(), vec!["a".to_string(), "b".to_string()]);
    }
}
