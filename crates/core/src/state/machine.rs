//! FSM facade trait + a bare in-memory implementation.
//!
//! The trait is the contract every adapter (statig today, rust-fsm or any
//! future variant later) implements. `inv-bus` wires the veto / emit hooks
//! around this trait in T-0014 — this module deliberately knows nothing
//! about the bus.
//!
//! Shape:
//!
//! ```ignore
//! pub trait StateMachine<S, E> {
//!     fn current_state(&self) -> S;
//!     fn propose(&self, e: &E) -> Result<S, VetoError>;
//!     fn apply(&mut self, e: &E) -> Result<S, TransitionError>;
//! }
//! ```
//!
//! `propose` is non-mutating: it consults the transition table to compute
//! the next state and returns it (or a [`VetoError`]). It's the seam where
//! T-0014 will publish `inv.billing.invoice.proposed` and check for
//! subscriber vetoes before the caller commits.
//!
//! `apply` mutates: it runs the transition table and updates the machine's
//! current state. T-0014 will emit `.transitioned` and `.entered` after a
//! successful `apply`.

use crate::domain::invoice::InvoiceState;
use crate::state::transitions::{next_state, InvoiceEvent, TransitionError};
use thiserror::Error;

/// Generic FSM facade trait.
///
/// `S` = state type, `E` = event type. The default `propose` implementation
/// covers the no-subscriber case: any failure from the transition table
/// surfaces as [`VetoError::Illegal`]. Subscribers (T-0014) will be injected
/// by wrapping `propose` in the commands layer; this trait does not own the
/// bus.
pub trait StateMachine<S, E> {
    /// Returns the current state. Cheap; states are `Copy` for our use case.
    fn current_state(&self) -> S;

    /// Compute the post-event state WITHOUT mutating.
    ///
    /// Default impl delegates to [`StateMachine::dry_run`]; implementors only
    /// override when they need to add side effects (T-0014 wiring).
    fn propose(&self, e: &E) -> Result<S, VetoError>
    where
        S: Copy,
    {
        self.dry_run(e).map_err(VetoError::Illegal)
    }

    /// Compute the post-event state WITHOUT mutating, returning the raw
    /// transition error verbatim. Used by [`Self::propose`]'s default impl
    /// and by tests that want to assert on the underlying [`TransitionError`].
    fn dry_run(&self, e: &E) -> Result<S, TransitionError>;

    /// Run the transition AND mutate `self`'s current state on success.
    fn apply(&mut self, e: &E) -> Result<S, TransitionError>;
}

/// Why a proposed transition was rejected.
///
/// Two reasons:
///
/// - [`VetoError::Illegal`] — the transition table itself rejected the pair.
///   Same payload as [`TransitionError`].
/// - [`VetoError::Vetoed`] — a subscriber said "no" on the bus seam (T-0014).
///   Carries a free-form reason for audit and surfacing.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VetoError {
    /// The transition is illegal per the state machine's table.
    #[error(transparent)]
    Illegal(#[from] TransitionError),
    /// A bus subscriber vetoed the proposal.
    #[error("transition vetoed: {0}")]
    Vetoed(String),
}

/// A bare, in-memory implementation of [`StateMachine`] for the invoice FSM.
///
/// Backed only by the transition table in [`crate::state::transitions`]. No
/// hierarchy, no entry/exit actions, no bus wiring — useful as a baseline
/// against which the statig adapter is parity-tested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BareMachine {
    state: InvoiceState,
}

impl BareMachine {
    /// New machine seeded with the given state (typically `Draft` for a
    /// fresh invoice, or whatever was reloaded from the DB).
    pub fn new(initial: InvoiceState) -> Self {
        Self { state: initial }
    }
}

impl StateMachine<InvoiceState, InvoiceEvent> for BareMachine {
    fn current_state(&self) -> InvoiceState {
        self.state
    }

    fn dry_run(&self, e: &InvoiceEvent) -> Result<InvoiceState, TransitionError> {
        next_state(self.state, e)
    }

    fn apply(&mut self, e: &InvoiceEvent) -> Result<InvoiceState, TransitionError> {
        let next = next_state(self.state, e)?;
        self.state = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::FromStr;
    use rust_decimal::Decimal;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn pay(amount: &str, total: &str) -> InvoiceEvent {
        InvoiceEvent::Pay {
            amount_paid: dec(amount),
            total: dec(total),
        }
    }

    #[test]
    fn bare_machine_walks_happy_path() {
        let mut m = BareMachine::new(InvoiceState::Draft);
        assert_eq!(m.current_state(), InvoiceState::Draft);

        assert_eq!(m.apply(&InvoiceEvent::Issue).unwrap(), InvoiceState::Issued);
        assert_eq!(m.apply(&InvoiceEvent::Send).unwrap(), InvoiceState::Sent);
        assert_eq!(m.apply(&InvoiceEvent::View).unwrap(), InvoiceState::Viewed);
        assert_eq!(
            m.apply(&pay("100.00", "100.00")).unwrap(),
            InvoiceState::Paid
        );
        assert_eq!(m.current_state(), InvoiceState::Paid);
    }

    #[test]
    fn bare_machine_remind_self_edge_does_not_change_state() {
        let mut m = BareMachine::new(InvoiceState::Sent);
        assert_eq!(m.apply(&InvoiceEvent::Remind).unwrap(), InvoiceState::Sent);
        assert_eq!(m.current_state(), InvoiceState::Sent);
    }

    #[test]
    fn propose_does_not_mutate() {
        let m = BareMachine::new(InvoiceState::Draft);
        assert_eq!(
            m.propose(&InvoiceEvent::Issue).unwrap(),
            InvoiceState::Issued
        );
        // Still Draft after a non-mutating propose.
        assert_eq!(m.current_state(), InvoiceState::Draft);
    }

    #[test]
    fn propose_surfaces_illegal_as_veto_error() {
        let m = BareMachine::new(InvoiceState::Draft);
        let err = m.propose(&pay("50.00", "100.00")).unwrap_err();
        match err {
            VetoError::Illegal(TransitionError::Illegal { event: "pay", .. }) => {}
            other => panic!("expected Illegal(pay), got {other:?}"),
        }
    }

    #[test]
    fn apply_returns_transition_error_on_illegal() {
        let mut m = BareMachine::new(InvoiceState::Paid);
        let err = m.apply(&InvoiceEvent::Void).unwrap_err();
        assert!(matches!(err, TransitionError::Illegal { .. }));
        // State must not advance on failure.
        assert_eq!(m.current_state(), InvoiceState::Paid);
    }

    #[test]
    fn partial_then_remainder_lands_in_paid() {
        let mut m = BareMachine::new(InvoiceState::Sent);
        assert_eq!(
            m.apply(&pay("40.00", "100.00")).unwrap(),
            InvoiceState::PartiallyPaid
        );
        // amount_paid here is the cumulative amount (40 + 60 = 100).
        assert_eq!(
            m.apply(&pay("100.00", "100.00")).unwrap(),
            InvoiceState::Paid
        );
    }

    #[test]
    fn veto_error_display_for_illegal() {
        let m = BareMachine::new(InvoiceState::Paid);
        let err = m.propose(&InvoiceEvent::Void).unwrap_err();
        // The transparent inner error should propagate through Display.
        assert!(err.to_string().contains("illegal transition"));
    }

    #[test]
    fn veto_error_display_for_vetoed() {
        let err = VetoError::Vetoed("nope".into());
        assert_eq!(err.to_string(), "transition vetoed: nope");
    }
}
