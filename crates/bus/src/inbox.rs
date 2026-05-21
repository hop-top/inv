//! Idempotent inbox.
//!
//! Inbound bus events are de-duped through `bus_inbox`: an INSERT-or-skip
//! on the (immutable) `event_id` primary key. [`dispatch_inbound_event`]
//! returns `true` when the event is fresh — the caller dispatches the
//! actual command — and `false` when the event has already been seen.
//!
//! The function does NOT execute any business logic itself. Mapping
//! topics to commands is a router concern (the consumer adapter / WS
//! gateway / API handler stitches that together); the inbox guarantees
//! every command is executed at most once per `event_id`.

use chrono::Utc;

use inv_store::repo::bus_inbox::{BusInboxRecord, BusInboxRepo};

use crate::error::IngestError;

/// Register an inbound event in `bus_inbox`. Returns:
///
/// - `Ok(true)` — the event was inserted now and the caller SHOULD
///   proceed to dispatch the matching command.
/// - `Ok(false)` — the event was already in `bus_inbox`; this is a
///   replay. Caller SHOULD skip dispatch.
/// - `Err(_)` — store-level failure (the caller MUST retry).
///
/// Note: `dispatch_inbound_event` doesn't itself call `mark_processed`.
/// The router does that after the command completes (so a crash between
/// insert and command-completion leaves the row in "received but not
/// processed" state, which an admin tooling can replay).
pub async fn dispatch_inbound_event(
    repo: &BusInboxRepo<'_>,
    event_id: &str,
    topic: &str,
    source: &str,
    payload: &str,
) -> Result<bool, IngestError> {
    let rec = BusInboxRecord {
        event_id: event_id.to_string(),
        topic: topic.to_string(),
        source: source.to_string(),
        received_at: Utc::now(),
        payload_json: payload.to_string(),
        processed_at: None,
        invoice_id: None,
    };
    let inserted = repo.try_insert(&rec).await?;
    Ok(inserted)
}
