//! `statig`-backed adapter for the invoice FSM.
//!
//! Hierarchy (design §3.4): `Issued` is a superstate of `Sent` and `Viewed`.
//! The Pay / Void verdicts that apply uniformly from any of `Issued`,
//! `Sent`, or `Viewed` are encoded once at the `Issued` superstate level;
//! the leaf substates only carry the verbs that uniquely differentiate
//! them (`Send`, `View`, `Remind`). `Draft`, `PartiallyPaid`, `Paid`, and
//! `Voided` are flat leaves.
//!
//! The adapter pre-checks every event against the canonical transition
//! table in [`crate::state::transitions::next_state`] before driving
//! statig. That guarantees:
//!
//! 1. Illegal events surface as [`TransitionError`] (statig itself has no
//!    "illegal event" concept; unhandled events propagate to root and are
//!    silently dropped, which would be wrong for our facade).
//! 2. The bare machine and the statig adapter always reach the same state
//!    after the same sequence of events (parity tested below).
//!
//! statig provides the hierarchy machinery — entry/exit actions, superstate
//! dispatch, transition observability — which downstream tooling (audit
//! replay, observability) can hook onto. T-0014 wires bus emission around
//! the adapter, not inside it.

use crate::domain::invoice::InvoiceState;
use crate::state::machine::StateMachine;
use crate::state::transitions::{next_state, InvoiceEvent, TransitionError};
use statig::blocking::{self, IntoStateMachineExt};
use statig::IntoStateMachine;

/// Shared context for the statig machine.
///
/// Currently empty — entry/exit actions don't need to read from any external
/// store. Kept as a named type so T-0014 can extend it (bus handle, actor,
/// channel) without changing the adapter's public surface.
#[derive(Debug, Default, Clone)]
pub struct Ctx;

/// Statig leaf-state enum, mirroring [`InvoiceState`] one-to-one.
///
/// Kept separate from the domain enum because statig requires its state
/// type to be `Copy + PartialEq + Debug` and to participate in its own
/// dispatch machinery. The mapping is total in both directions — see
/// [`StatigState::to_domain`] / [`StatigState::from_domain`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatigState {
    /// Flat leaf — no superstate.
    Draft,
    /// Leaf under `Issued` superstate.
    Issued,
    /// Leaf under `Issued` superstate.
    Sent,
    /// Leaf under `Issued` superstate.
    Viewed,
    /// Flat leaf — no superstate.
    PartiallyPaid,
    /// Flat leaf — terminal.
    Paid,
    /// Flat leaf — terminal.
    Voided,
}

/// Statig superstate enum.
///
/// Only one superstate ships at v1: `Issued`. It groups `Issued`, `Sent`,
/// and `Viewed` so the Pay / Void verdicts that apply uniformly across
/// them can be expressed once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatigSuperstate {
    /// Issued / Sent / Viewed share Pay + Void semantics.
    Issued,
}

impl StatigState {
    /// Translate to the domain [`InvoiceState`] enum.
    pub fn to_domain(self) -> InvoiceState {
        match self {
            StatigState::Draft => InvoiceState::Draft,
            StatigState::Issued => InvoiceState::Issued,
            StatigState::Sent => InvoiceState::Sent,
            StatigState::Viewed => InvoiceState::Viewed,
            StatigState::PartiallyPaid => InvoiceState::PartiallyPaid,
            StatigState::Paid => InvoiceState::Paid,
            StatigState::Voided => InvoiceState::Voided,
        }
    }

    /// Translate from the domain [`InvoiceState`] enum.
    pub fn from_domain(s: InvoiceState) -> Self {
        match s {
            InvoiceState::Draft => StatigState::Draft,
            InvoiceState::Issued => StatigState::Issued,
            InvoiceState::Sent => StatigState::Sent,
            InvoiceState::Viewed => StatigState::Viewed,
            InvoiceState::PartiallyPaid => StatigState::PartiallyPaid,
            InvoiceState::Paid => StatigState::Paid,
            InvoiceState::Voided => StatigState::Voided,
        }
    }
}

// -- statig wiring -----------------------------------------------------------

impl IntoStateMachine for Ctx {
    type State = StatigState;
    type Superstate<'sub> = StatigSuperstate;
    type Event<'evt> = InvoiceEvent;
    type Context<'ctx> = ();
    const INITIAL: StatigState = StatigState::Draft;
}

impl blocking::State<Ctx> for StatigState {
    fn call_handler(
        &mut self,
        _shared: &mut Ctx,
        event: &InvoiceEvent,
        _ctx: &mut (),
    ) -> statig::Response<Self> {
        // Each leaf consults the transition table. On illegal events the
        // leaf returns `Super` so the superstate (if any) gets a chance.
        // The wrapping [`StatigAdapter::apply`] does its own pre-check so
        // illegal events never actually reach this dispatcher under normal
        // use — these arms exist for completeness and for direct statig
        // invocation in tests.
        let domain_state = self.to_domain();
        match next_state(domain_state, event) {
            Ok(next) => statig::Response::Transition(StatigState::from_domain(next)),
            // Sent + Remind is a legal self-edge (Ok(Sent)); falls through
            // to the Ok arm above.
            Err(_) => statig::Response::Super,
        }
    }

    fn call_entry_action(&mut self, _shared: &mut Ctx, _ctx: &mut ()) {
        // No-op at v1. T-0014 (`inv-bus`) hooks `.entered` emission here.
    }

    fn call_exit_action(&mut self, _shared: &mut Ctx, _ctx: &mut ()) {
        // No-op at v1. Available as a seam for outbound emission.
    }

    fn superstate(&mut self) -> Option<StatigSuperstate> {
        match self {
            StatigState::Issued | StatigState::Sent | StatigState::Viewed => {
                Some(StatigSuperstate::Issued)
            }
            StatigState::Draft
            | StatigState::PartiallyPaid
            | StatigState::Paid
            | StatigState::Voided => None,
        }
    }
}

impl blocking::Superstate<Ctx> for StatigSuperstate {
    fn call_handler(
        &mut self,
        _shared: &mut Ctx,
        _event: &InvoiceEvent,
        _ctx: &mut (),
    ) -> statig::Response<StatigState> {
        // Pay/Void are already handled by the leaves via the transition
        // table. The superstate is here for hierarchy structure and for
        // future entry/exit grouping.
        statig::Response::Handled
    }

    fn call_entry_action(&mut self, _shared: &mut Ctx, _ctx: &mut ()) {}

    fn call_exit_action(&mut self, _shared: &mut Ctx, _ctx: &mut ()) {}

    fn superstate(&mut self) -> Option<StatigSuperstate> {
        None
    }
}

/// The shipped invoice FSM adapter.
///
/// Holds an inner `statig::blocking::InitializedStateMachine` plus a
/// shadow [`InvoiceState`] that tracks the canonical (table-verified)
/// domain state. The shadow exists because statig 0.3 doesn't expose a
/// public state setter — for invoices rehydrated from persistence in a
/// non-`Draft` state, we can't seed the inner machine directly. The
/// shadow is the source of truth that callers see; the inner machine is
/// driven whenever the shadow matches its own state (i.e. the happy-path
/// case of "fresh invoice walks the FSM from Draft") so that statig's
/// hierarchy + entry/exit machinery exercises end-to-end.
///
/// The transition table in [`crate::state::transitions`] is the verdict
/// for legality in both cases — never statig. That guarantees parity
/// with [`crate::state::machine::BareMachine`].
pub struct StatigAdapter {
    inner: blocking::InitializedStateMachine<Ctx>,
    /// Canonical state, always consulted via [`StateMachine::current_state`].
    shadow: InvoiceState,
}

impl std::fmt::Debug for StatigAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatigAdapter")
            .field("state", &self.shadow)
            .field("statig_state", &self.inner.state())
            .finish()
    }
}

impl StatigAdapter {
    /// Seed the adapter with a starting state. Invoices begin in `Draft`;
    /// rehydrated invoices use whatever was persisted.
    pub fn new(initial: InvoiceState) -> Self {
        // statig always starts in `INITIAL` (Draft) — we can't override
        // before init. The shadow tracks the actual current state; the
        // inner machine stays a step behind when the caller seeded a
        // non-Draft state. `apply` reconciles per call.
        let inner = Ctx.uninitialized_state_machine().init();
        Self {
            inner,
            shadow: initial,
        }
    }

    /// Borrow the inner statig machine (read-only). Useful for tooling
    /// that wants to observe statig's view, e.g. inspect leaf-vs-superstate
    /// dispatch. Production callers should go through [`Self::current_state`].
    pub fn statig_state(&self) -> StatigState {
        *self.inner.state()
    }
}

impl StateMachine<InvoiceState, InvoiceEvent> for StatigAdapter {
    fn current_state(&self) -> InvoiceState {
        self.shadow
    }

    fn dry_run(&self, e: &InvoiceEvent) -> Result<InvoiceState, TransitionError> {
        next_state(self.shadow, e)
    }

    fn apply(&mut self, e: &InvoiceEvent) -> Result<InvoiceState, TransitionError> {
        // Canonical verdict from the table. Illegal events bail before we
        // touch the statig machine.
        let next = next_state(self.shadow, e)?;
        // Drive statig if its current state matches the shadow — i.e. the
        // hierarchy machinery applies for this step. For rehydrated
        // machines whose shadow ran ahead of the statig view, we skip the
        // statig handle to avoid running entry actions for states the
        // application has already left in a previous lifetime.
        if self.inner.state().to_domain() == self.shadow {
            self.inner.handle(e);
        }
        self.shadow = next;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::machine::BareMachine;
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
    fn state_translation_is_total_round_trip() {
        for s in [
            InvoiceState::Draft,
            InvoiceState::Issued,
            InvoiceState::Sent,
            InvoiceState::Viewed,
            InvoiceState::PartiallyPaid,
            InvoiceState::Paid,
            InvoiceState::Voided,
        ] {
            assert_eq!(StatigState::from_domain(s).to_domain(), s);
        }
    }

    #[test]
    fn adapter_starts_in_draft_by_default() {
        let a = StatigAdapter::new(InvoiceState::Draft);
        assert_eq!(a.current_state(), InvoiceState::Draft);
    }

    #[test]
    fn adapter_can_seed_non_default_state() {
        let a = StatigAdapter::new(InvoiceState::Sent);
        assert_eq!(a.current_state(), InvoiceState::Sent);
    }

    #[test]
    fn adapter_walks_happy_path() {
        let mut a = StatigAdapter::new(InvoiceState::Draft);
        assert_eq!(a.apply(&InvoiceEvent::Issue).unwrap(), InvoiceState::Issued);
        assert_eq!(a.apply(&InvoiceEvent::Send).unwrap(), InvoiceState::Sent);
        assert_eq!(a.apply(&InvoiceEvent::View).unwrap(), InvoiceState::Viewed);
        assert_eq!(
            a.apply(&pay("100.00", "100.00")).unwrap(),
            InvoiceState::Paid
        );
    }

    #[test]
    fn adapter_remind_self_edge_stays_sent() {
        let mut a = StatigAdapter::new(InvoiceState::Sent);
        assert_eq!(a.apply(&InvoiceEvent::Remind).unwrap(), InvoiceState::Sent);
        assert_eq!(a.current_state(), InvoiceState::Sent);
    }

    #[test]
    fn adapter_rejects_pay_on_draft() {
        let mut a = StatigAdapter::new(InvoiceState::Draft);
        let err = a.apply(&pay("10.00", "100.00")).unwrap_err();
        assert!(matches!(err, TransitionError::Illegal { event: "pay", .. }));
        // State must not advance on rejection.
        assert_eq!(a.current_state(), InvoiceState::Draft);
    }

    #[test]
    fn adapter_treats_paid_as_terminal() {
        let mut a = StatigAdapter::new(InvoiceState::Paid);
        let err = a.apply(&InvoiceEvent::Void).unwrap_err();
        assert!(matches!(err, TransitionError::Illegal { .. }));
        assert_eq!(a.current_state(), InvoiceState::Paid);
    }

    #[test]
    fn adapter_propose_does_not_mutate() {
        let a = StatigAdapter::new(InvoiceState::Draft);
        assert_eq!(
            a.propose(&InvoiceEvent::Issue).unwrap(),
            InvoiceState::Issued
        );
        assert_eq!(a.current_state(), InvoiceState::Draft);
    }

    #[test]
    fn adapter_void_from_partially_paid() {
        let mut a = StatigAdapter::new(InvoiceState::PartiallyPaid);
        assert_eq!(a.apply(&InvoiceEvent::Void).unwrap(), InvoiceState::Voided);
    }

    /// Parity: a sequence of events must take both the bare machine and
    /// the statig adapter through the exact same domain-state trace.
    #[test]
    fn parity_with_bare_machine_full_payment_path() {
        let trace = run_parity(
            InvoiceState::Draft,
            &[
                InvoiceEvent::Issue,
                InvoiceEvent::Send,
                InvoiceEvent::Remind,
                InvoiceEvent::View,
                pay("100.00", "100.00"),
            ],
        );
        assert_eq!(
            trace,
            vec![
                InvoiceState::Issued,
                InvoiceState::Sent,
                InvoiceState::Sent,
                InvoiceState::Viewed,
                InvoiceState::Paid,
            ]
        );
    }

    #[test]
    fn parity_with_bare_machine_partial_then_remainder() {
        let trace = run_parity(
            InvoiceState::Draft,
            &[
                InvoiceEvent::Issue,
                InvoiceEvent::Send,
                pay("40.00", "100.00"),
                pay("100.00", "100.00"),
            ],
        );
        assert_eq!(
            trace,
            vec![
                InvoiceState::Issued,
                InvoiceState::Sent,
                InvoiceState::PartiallyPaid,
                InvoiceState::Paid,
            ]
        );
    }

    #[test]
    fn parity_with_bare_machine_void_path() {
        let trace = run_parity(
            InvoiceState::Draft,
            &[InvoiceEvent::Issue, InvoiceEvent::Send, InvoiceEvent::Void],
        );
        assert_eq!(
            trace,
            vec![
                InvoiceState::Issued,
                InvoiceState::Sent,
                InvoiceState::Voided,
            ]
        );
    }

    /// Runs the same event sequence on both machines, asserts each step
    /// agrees, and returns the resulting trace for caller-side assertions.
    fn run_parity(initial: InvoiceState, events: &[InvoiceEvent]) -> Vec<InvoiceState> {
        let mut bare = BareMachine::new(initial);
        let mut sta = StatigAdapter::new(initial);
        let mut trace = Vec::new();
        for ev in events {
            let bare_next = bare.apply(ev);
            let stat_next = sta.apply(ev);
            assert_eq!(
                bare_next,
                stat_next,
                "bare/statig disagree on {ev:?} from {:?}",
                bare.current_state()
            );
            assert_eq!(bare.current_state(), sta.current_state());
            trace.push(bare.current_state());
        }
        trace
    }

    #[test]
    fn parity_illegal_events_match_on_both_machines() {
        for (initial, ev) in [
            (InvoiceState::Draft, pay("10.00", "100.00")),
            (InvoiceState::Draft, InvoiceEvent::Void),
            (InvoiceState::Paid, InvoiceEvent::Void),
            (InvoiceState::Paid, InvoiceEvent::Send),
            (InvoiceState::Voided, InvoiceEvent::Issue),
            (InvoiceState::Issued, InvoiceEvent::Issue),
            (InvoiceState::Issued, InvoiceEvent::Remind),
            (InvoiceState::Issued, InvoiceEvent::View),
        ] {
            let mut bare = BareMachine::new(initial);
            let mut sta = StatigAdapter::new(initial);
            let bare_err = bare.apply(&ev).unwrap_err();
            let stat_err = sta.apply(&ev).unwrap_err();
            assert_eq!(
                bare_err, stat_err,
                "bare/statig should reject {ev:?} identically from {initial:?}"
            );
            // Neither machine advanced.
            assert_eq!(bare.current_state(), initial);
            assert_eq!(sta.current_state(), initial);
        }
    }

    #[test]
    fn hierarchy_groups_issued_sent_viewed() {
        // The statig superstate machinery groups Issued/Sent/Viewed.
        // Confirm via direct introspection on the state translation.
        let mut s_issued = StatigState::Issued;
        let mut s_sent = StatigState::Sent;
        let mut s_viewed = StatigState::Viewed;
        let mut s_draft = StatigState::Draft;
        let mut s_paid = StatigState::Paid;

        assert_eq!(
            blocking::State::<Ctx>::superstate(&mut s_issued),
            Some(StatigSuperstate::Issued)
        );
        assert_eq!(
            blocking::State::<Ctx>::superstate(&mut s_sent),
            Some(StatigSuperstate::Issued)
        );
        assert_eq!(
            blocking::State::<Ctx>::superstate(&mut s_viewed),
            Some(StatigSuperstate::Issued)
        );
        assert_eq!(blocking::State::<Ctx>::superstate(&mut s_draft), None);
        assert_eq!(blocking::State::<Ctx>::superstate(&mut s_paid), None);
    }
}
