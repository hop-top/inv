//! [`EmittedEvent`] — the envelope each command returns for every event
//! it would publish on the bus.
//!
//! At v1 nothing actually publishes; T-0014 (`inv-bus`) will consume
//! these envelopes and write them onto `hop-top-kit`'s bus. Returning
//! them in the command output keeps the test harness easy to assert
//! against without spinning up the bus stack.

use chrono::{DateTime, Utc};
use serde_json::Value;

/// One bus event that a command would emit.
///
/// `topic` is the dotted topic name (e.g.
/// `inv.billing.invoice.issued`), `payload` is the JSON body, and
/// `emitted_at` is the wall-clock the command captured at emit time
/// (taken from the injected clock so tests can pin it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmittedEvent {
    /// Dotted topic name.
    pub topic: String,
    /// Event payload (JSON).
    pub payload: Value,
    /// Wall-clock at emit time.
    pub emitted_at: DateTime<Utc>,
}

impl EmittedEvent {
    /// Construct.
    pub fn new(topic: impl Into<String>, payload: Value, emitted_at: DateTime<Utc>) -> Self {
        Self {
            topic: topic.into(),
            payload,
            emitted_at,
        }
    }
}
