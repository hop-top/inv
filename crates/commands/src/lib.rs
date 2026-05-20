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
//! - returns the list of bus events it WOULD emit, in
//!   `EmittedEvent` form. T-0014 will wire those onto the real bus.
//!
//! ## Module layout
//!
//! - [`ctx`] — [`CoreCtx`] + [`Actor`] + [`Channel`] (channel mirror of
//!   `inv_core::domain::invoice::HistoryChannel`).
//! - [`error`] — [`CoreError`] variants surfaced to every channel.
//! - [`events`] — [`EmittedEvent`] envelope returned in command outputs.
//! - [`draft`] — `draft_invoice`.
//! - [`issue`] — `issue_invoice`.
//! - [`send`] — `send_invoice`.

#![deny(missing_docs)]

pub mod ctx;
pub mod draft;
pub mod error;
pub mod events;
pub mod issue;
pub mod send;

pub use ctx::{Actor, Channel, Clock, CoreCtx, SystemClock};
pub use draft::{
    draft_invoice, DraftInvoiceInput, DraftInvoiceOutput, DraftLineInput,
};
pub use error::CoreError;
pub use events::EmittedEvent;
pub use issue::{issue_invoice, IssueInvoiceInput, IssueInvoiceOutput};
pub use send::{
    send_invoice, SendInvoiceInput, SendInvoiceOutput, SendSink,
};
