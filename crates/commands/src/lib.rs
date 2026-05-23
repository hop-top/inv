//! inv-commands — the one-command-core layer that every channel adapter
//! (CLI, API, WS, MCP, bus consumer) calls into.
//!
//! Per design spec §3.1, every channel reduces to the same set of
//! commands; this crate hosts those commands and the dependency bundle
//! ([`CoreCtx`]) they consume.
//!
//! ## Why a separate crate?
//!
//! Commands need both the domain/FSM/render/tax modules of `inv-core`
//! AND the repository structs of `inv-store`. `inv-store` already
//! depends on `inv-core` for domain types — putting commands inside
//! `inv-core` would create a cycle. Splitting them out keeps the graph
//! acyclic and lets every channel adapter (which also depends on both)
//! reuse the exact same code paths.
//!
//! ## Cross-cutting invariants (design §3.5)
//!
//! Every command:
//! - validates its input up front (rejects empty lines, non-positive
//!   quantities, mismatched currency, etc.);
//! - checks `idempotency_key` BEFORE mutating — a prior invoice with
//!   the same key returns the same output unchanged;
//! - opens a single sqlx transaction encompassing the mutation +
//!   `invoice_state_history` insert (the history row doubles as the
//!   outbox, `published_at = NULL` signals pending);
//! - calls `publisher::try_publish` (T-0043) when `ctx.publisher` is
//!   wired so subscribers see the event synchronously. The
//!   history-row outbox remains the canonical record — the relay in
//!   `crates/bus/src/outbox.rs` replays anything the sync publish
//!   missed.
//!
//! ## Module layout
//!
//! - [`ctx`] — [`CoreCtx`] + [`Actor`] + [`Channel`] (channel mirror of
//!   `inv_core::domain::invoice::HistoryChannel`).
//! - [`error`] — [`CoreError`] variants surfaced to every channel.
//! - [`publisher`] — [`Publisher`] trait + `try_publish` helper.
//! - [`draft`] — `draft_invoice`.
//! - [`issue`] — `issue_invoice`.
//! - [`send`] — `send_invoice`.

#![deny(missing_docs)]

pub mod credit;
pub mod ctx;
pub mod draft;
pub mod error;
pub mod issue;
pub mod overdue;
pub mod pay;
pub mod publisher;
pub mod reminder;
pub mod schedule;
pub mod send;
pub mod void;

pub use credit::{
    create_credit_note, issue_credit_note, CreateCreditNoteInput, CreateCreditNoteOutput,
    IssueCreditNoteInput, IssueCreditNoteOutput,
};
pub use ctx::{Actor, Channel, Clock, CoreCtx, SystemClock};
pub use draft::{draft_invoice, DraftInvoiceInput, DraftInvoiceOutput, DraftLineInput};
pub use error::CoreError;
pub use issue::{issue_invoice, IssueInvoiceInput, IssueInvoiceOutput};
pub use overdue::{mark_overdue_ticker, OverdueTickerOutput};
pub use pay::{mark_paid, MarkPaidInput, MarkPaidOutput};
pub use publisher::{PublishError, Publisher};
pub use reminder::{
    reminder_cancel, reminder_schedule, reminders_tick, ReminderCancelInput, ReminderCancelOutput,
    ReminderScheduleInput, ReminderScheduleOutput, RemindersTickOutput,
};
pub use schedule::{
    schedule_cancel, schedule_create, schedule_pause, schedules_tick, ScheduleCreateInput,
    ScheduleCreateOutput, ScheduleLineInput, ScheduleStateChangeInput, ScheduleStateChangeOutput,
    SchedulesTickOutput,
};
pub use send::{
    send_invoice, send_invoice_render, SendInvoiceInput, SendInvoiceOutput, SendInvoiceRenderInput,
    SendSink,
};
pub use void::{void_invoice, VoidInvoiceInput, VoidInvoiceOutput};
