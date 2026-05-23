//! Invoice FSM facade.
//!
//! Per design spec §3.3 / §3.4: this module owns the lifecycle state
//! machine for invoices. It exposes a [`machine::StateMachine`] trait, a
//! pure transition table ([`transitions::next_state`]), and a single
//! shipped adapter ([`adapter_statig::StatigAdapter`]) backed by the
//! `statig` crate.
//!
//! Layout:
//!
//! - [`transitions`] — exhaustive `match`-based transition table. Source
//!   of truth for legality.
//! - [`machine`] — `StateMachine` trait + `BareMachine` baseline impl.
//! - [`adapter_statig`] — statig-backed adapter (hierarchical: Issued >
//!   {Sent, Viewed}).
//! - [`events`] — bus event shapes for the `.proposed` / `.transitioned`
//!   / `.entered` mechanic-event triplet.
//!
//! No bus wiring lives here. T-0014 (`hop-top-inv-bus`) consumes the
//! [`events`] structs and runs the [`machine::VetoError`] seam around
//! [`machine::StateMachine::propose`].

pub mod adapter_statig;
pub mod creditnote;
pub mod events;
pub mod machine;
pub mod transitions;

// Re-exports for ergonomic `use hop_top_inv_core::state::*;`.
pub use adapter_statig::StatigAdapter;
pub use events::{
    InvoiceEntered, InvoiceProposed, InvoiceTransitioned, TOPIC_ENTERED, TOPIC_PROPOSED,
    TOPIC_TRANSITIONED,
};
pub use machine::{BareMachine, StateMachine, VetoError};
pub use transitions::{
    classify_payment, is_self_edge, next_state, InvoiceEvent, PaymentKind, TransitionError,
};
