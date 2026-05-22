//! inv-bus — bus event types, publisher/consumer adapters, transactional
//! outbox relay, idempotent inbox.
//!
//! ## Why a local facade
//!
//! kit's Rust SDK does not yet ship a bus primitive (tracked by
//! `hop-top/poly-kit#sdk-rs-runtime#T-0759`). This crate wraps a local
//! [`Publisher`] trait now so the rest of inv (commands, ws, api, server
//! boot) can depend on a stable bus surface; when kit's primitive lands,
//! the trait's concrete impls swap to a kit-backed publisher with no
//! caller changes.
//!
//! ## Layout
//!
//! - [`events`] — typed payload structs for every domain event listed in
//!   design §4.2 (`drafted`, `issued`, `sent`, `viewed`, `paid`,
//!   `partially_paid`, `overdue`, `voided`, `creditnote.drafted` /
//!   `.issued`, reminder + schedule lifecycle). Mechanic-event payloads
//!   (`InvoiceProposed` / `InvoiceTransitioned` / `InvoiceEntered` and
//!   the credit-note triplet) are re-exported from `inv-core::state`.
//! - [`publisher`] — [`Publisher`] trait + [`LoggingPublisher`] (boot
//!   default) + [`InMemoryPublisher`] (tests).
//! - [`broadcast`] — [`BroadcastPublisher`] — in-process fanout
//!   [`Publisher`] used by the WebSocket adapter so per-connection
//!   subscribers can pull the same event stream the outbox emits.
//! - [`outbox`] — [`run_outbox_relay`]. Polls
//!   `invoice_state_history` + `credit_note_state_history` for rows with
//!   `published_at IS NULL`, reconstructs the mechanic triplet + the
//!   domain event from each row, publishes them, marks the row.
//! - [`inbox`] — [`dispatch_inbound_event`]. Inserts into `bus_inbox`;
//!   returns `true` on a fresh insertion and `false` on a dedup hit.
//! - [`topic_map`] — pure config-driven topic remap (design §4.3).
//! - [`error`] — error types for publish / relay / ingest.

#![deny(missing_docs)]

pub mod broadcast;
pub mod consumer;
pub mod error;
pub mod events;
pub mod inbox;
pub mod outbox;
pub mod publisher;
pub mod topic_map;

pub use broadcast::{BroadcastPublisher, BroadcastSubscriber, BusMessage};
pub use consumer::{Consumer, DispatchError, DispatchOutput};
pub use error::{IngestError, PublishError, RelayError};
pub use events::{
    CreditNoteDrafted, CreditNoteIssued, InvoiceDrafted, InvoiceIssued, InvoiceOverdue,
    InvoicePaid, InvoicePartiallyPaid, InvoiceSent, InvoiceViewed, InvoiceVoided,
    ReminderCancelled, ReminderScheduled, ReminderSent, ScheduleCancelled, ScheduleCreated,
    SchedulePaused, TOPIC_CREDITNOTE_DRAFTED, TOPIC_CREDITNOTE_ISSUED, TOPIC_INVOICE_DRAFTED,
    TOPIC_INVOICE_ISSUED, TOPIC_INVOICE_OVERDUE, TOPIC_INVOICE_PAID,
    TOPIC_INVOICE_PARTIALLY_PAID, TOPIC_INVOICE_SENT, TOPIC_INVOICE_VIEWED, TOPIC_INVOICE_VOIDED,
    TOPIC_REMINDER_CANCELLED, TOPIC_REMINDER_SCHEDULED, TOPIC_REMINDER_SENT,
    TOPIC_SCHEDULE_CANCELLED, TOPIC_SCHEDULE_CREATED, TOPIC_SCHEDULE_PAUSED,
};
pub use inbox::dispatch_inbound_event;
pub use outbox::{run_outbox_relay, OutboxStats};
pub use publisher::{InMemoryPublisher, LoggingPublisher, Publisher};
pub use topic_map::{remap_topic, TopicMap};

// Mechanic-event payloads + topic constants are owned by inv-core::state;
// re-export so consumers of inv-bus get the full domain+mechanic surface
// from one crate.
pub use inv_core::state::creditnote::{
    CreditNoteEntered, CreditNoteProposed, CreditNoteTransitioned,
    TOPIC_ENTERED as TOPIC_CREDITNOTE_ENTERED, TOPIC_PROPOSED as TOPIC_CREDITNOTE_PROPOSED,
    TOPIC_TRANSITIONED as TOPIC_CREDITNOTE_TRANSITIONED,
};
pub use inv_core::state::events::{
    InvoiceEntered, InvoiceProposed, InvoiceTransitioned,
    TOPIC_ENTERED as TOPIC_INVOICE_ENTERED, TOPIC_PROPOSED as TOPIC_INVOICE_PROPOSED,
    TOPIC_TRANSITIONED as TOPIC_INVOICE_TRANSITIONED,
};
