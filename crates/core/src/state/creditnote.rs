//! Credit-note FSM facade.
//!
//! Per design spec §3.4: credit notes have their own mini-FSM,
//! `Draft → Issued`. `Issued` is terminal; no further transitions.
//!
//! This module mirrors the structure of the invoice FSM
//! ([`crate::state::transitions`], [`crate::state::machine`],
//! [`crate::state::adapter_statig`], [`crate::state::events`]) but is
//! collapsed into a single file because the FSM is small enough — 2 states
//! and 1 event — that splitting would obscure rather than clarify.
//!
//! Layout (logical, in this one file):
//!
//! - [`CreditNoteEvent`] + [`next_state`] — pure transition table.
//! - [`CreditNoteAdapter`] (the bare facade impl, equivalent of
//!   `BareMachine` for invoices) — implements
//!   [`crate::state::machine::StateMachine`] over [`CreditNoteState`] +
//!   [`CreditNoteEvent`].
//! - [`StatigAdapter`] — statig-backed adapter. The FSM is hierarchically
//!   flat (no superstates), so the wiring is one leaf-enum and one handler.
//! - [`CreditNoteProposed`] / [`CreditNoteTransitioned`] /
//!   [`CreditNoteEntered`] — bus event payloads + `TOPIC_*` constants
//!   matching the invoice mechanic-event triplet.
//!
//! Reuses [`crate::state::machine::StateMachine`],
//! [`crate::state::machine::VetoError`], and the shape of the invoice
//! [`crate::state::transitions::TransitionError`] (the credit-note variant
//! is local because the state type differs).
//!
//! No bus wiring lives here. T-0014 (`hop-top-inv-bus`) consumes the event structs
//! and runs the [`VetoError`] seam around
//! [`crate::state::machine::StateMachine::propose`].

use crate::domain::creditnote::CreditNoteState;
use crate::domain::ids::CreditNoteId;
use crate::domain::invoice::HistoryChannel;
use crate::state::machine::{StateMachine, VetoError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use statig::blocking::{self, IntoStateMachineExt};
use statig::IntoStateMachine;
use thiserror::Error;

// =============================================================================
// Transition table
// =============================================================================

/// FSM-level input event for credit notes.
///
/// Only one verb: [`CreditNoteEvent::Issue`] (`Draft -> Issued`). The enum is
/// kept open-shaped (rather than a unit struct) so future extensions
/// (`Void`, `Cancel`, etc.) can be added without rippling through callers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum CreditNoteEvent {
    /// Move `Draft -> Issued`. Assigns the human number on commit.
    Issue,
}

impl CreditNoteEvent {
    /// Short string tag used in audit rows and bus payloads.
    pub fn tag(&self) -> &'static str {
        match self {
            CreditNoteEvent::Issue => "issue",
        }
    }
}

/// Why a `(state, event)` pair was rejected by the credit-note transition
/// table.
///
/// Kept distinct from the invoice [`crate::state::transitions::TransitionError`]
/// because the carried `state` is [`CreditNoteState`], not `InvoiceState`. The
/// shape mirrors the invoice variant so tooling can treat them
/// interchangeably.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum TransitionError {
    /// The event has no legal effect from the given state.
    #[error("illegal transition: {event} not allowed from {state:?}")]
    Illegal {
        /// Current state at the time of the event.
        state: CreditNoteState,
        /// Event tag (see [`CreditNoteEvent::tag`]).
        event: &'static str,
    },
}

/// Resolve the next state for `(current, event)`.
///
/// Pure function. Does not mutate, emit, or touch the bus. Both the bare
/// facade impl ([`CreditNoteAdapter`]) and the statig adapter
/// ([`StatigAdapter`]) call into this so they stay in lock-step.
///
/// Per design spec §3.4:
///
/// - `Draft  + Issue -> Issued`
/// - `Issued + Issue -> Illegal` (terminal)
pub fn next_state(
    current: CreditNoteState,
    event: &CreditNoteEvent,
) -> Result<CreditNoteState, TransitionError> {
    use CreditNoteEvent as E;
    use CreditNoteState as S;

    match (current, event) {
        (S::Draft, E::Issue) => Ok(S::Issued),
        // Issued is terminal — every event is illegal.
        (S::Issued, _) => Err(illegal(current, event)),
    }
}

#[inline]
fn illegal(state: CreditNoteState, event: &CreditNoteEvent) -> TransitionError {
    TransitionError::Illegal {
        state,
        event: event.tag(),
    }
}

// =============================================================================
// Bare facade impl
// =============================================================================

/// Bare, in-memory implementation of [`StateMachine`] for credit notes.
///
/// Backed only by [`next_state`]. No hierarchy, no entry/exit actions, no
/// bus wiring — the baseline against which [`StatigAdapter`] is
/// parity-tested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreditNoteAdapter {
    state: CreditNoteState,
}

impl CreditNoteAdapter {
    /// New machine seeded with the given state (typically `Draft` for a
    /// fresh credit note, or whatever was reloaded from the DB).
    pub fn new(initial: CreditNoteState) -> Self {
        Self { state: initial }
    }
}

// The bare facade uses the credit-note-local `TransitionError`. Because the
// `StateMachine` trait is generic over its error type only through the
// `propose` default impl's `VetoError::Illegal` conversion, and `VetoError`'s
// `From<InvoiceTransitionError>` impl is invoice-specific, we provide our own
// `propose` override below.
impl StateMachine<CreditNoteState, CreditNoteEvent> for CreditNoteAdapter {
    fn current_state(&self) -> CreditNoteState {
        self.state
    }

    fn dry_run(
        &self,
        e: &CreditNoteEvent,
    ) -> Result<CreditNoteState, crate::state::transitions::TransitionError> {
        // The trait's `TransitionError` associated shape is the invoice one;
        // we never produce that variant from this machine, so we surface
        // illegal events through `apply`'s richer error type and only use
        // `dry_run` indirectly via the `propose` override below.
        unreachable!(
            "CreditNoteAdapter does not use the invoice TransitionError; use propose() or apply() — got event {:?}",
            e
        )
    }

    fn propose(&self, e: &CreditNoteEvent) -> Result<CreditNoteState, VetoError>
    where
        CreditNoteState: Copy,
    {
        // Delegate to the credit-note transition table; convert local
        // `TransitionError` to a `VetoError::Vetoed` carrying the
        // human-readable rejection. This keeps the trait usable for
        // credit-note callers without leaking the invoice-only `Illegal`
        // variant.
        match next_state(self.state, e) {
            Ok(s) => Ok(s),
            Err(err) => Err(VetoError::Vetoed(err.to_string())),
        }
    }

    fn apply(
        &mut self,
        e: &CreditNoteEvent,
    ) -> Result<CreditNoteState, crate::state::transitions::TransitionError> {
        // Same shape rationale as `dry_run`. Real callers go through
        // [`CreditNoteAdapter::apply_event`] which surfaces the credit-note
        // `TransitionError` verbatim. We keep the trait method available
        // (it'll never succeed via this entry point because the error type
        // doesn't match) by panicking on use; production code never calls it.
        unreachable!(
            "CreditNoteAdapter does not use the invoice TransitionError; call apply_event() — got event {:?}",
            e
        )
    }
}

impl CreditNoteAdapter {
    /// Apply an event, mutating the machine on success. The credit-note
    /// equivalent of [`StateMachine::apply`] with the credit-note-local
    /// error type.
    pub fn apply_event(&mut self, e: &CreditNoteEvent) -> Result<CreditNoteState, TransitionError> {
        let next = next_state(self.state, e)?;
        self.state = next;
        Ok(next)
    }

    /// Compute the post-event state without mutating, surfacing the
    /// credit-note-local error verbatim.
    pub fn dry_run_event(&self, e: &CreditNoteEvent) -> Result<CreditNoteState, TransitionError> {
        next_state(self.state, e)
    }
}

// =============================================================================
// statig adapter
// =============================================================================

/// Shared context for the statig credit-note machine.
///
/// Currently empty — entry/exit actions don't need to read from any external
/// store. Kept as a named type so T-0014 can extend it without changing
/// the adapter's public surface.
#[derive(Debug, Default, Clone)]
pub struct CnCtx;

/// Statig leaf-state enum, mirroring [`CreditNoteState`] one-to-one.
///
/// Kept separate from the domain enum because statig requires its state
/// type to be `Copy + PartialEq + Debug` and to participate in its own
/// dispatch machinery. The mapping is total in both directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatigState {
    /// Mutable.
    Draft,
    /// Frozen, numbered — terminal.
    Issued,
}

impl StatigState {
    /// Translate to the domain [`CreditNoteState`] enum.
    pub fn to_domain(self) -> CreditNoteState {
        match self {
            StatigState::Draft => CreditNoteState::Draft,
            StatigState::Issued => CreditNoteState::Issued,
        }
    }

    /// Translate from the domain [`CreditNoteState`] enum.
    pub fn from_domain(s: CreditNoteState) -> Self {
        match s {
            CreditNoteState::Draft => StatigState::Draft,
            CreditNoteState::Issued => StatigState::Issued,
        }
    }
}

/// Statig superstate enum. The credit-note FSM is hierarchically flat —
/// no superstates ship — but statig requires the type to exist. Marked
/// `#[allow(dead_code)]` because no variant is ever constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatigSuperstate {}

impl IntoStateMachine for CnCtx {
    type State = StatigState;
    type Superstate<'sub> = StatigSuperstate;
    type Event<'evt> = CreditNoteEvent;
    type Context<'ctx> = ();
    const INITIAL: StatigState = StatigState::Draft;
}

impl blocking::State<CnCtx> for StatigState {
    fn call_handler(
        &mut self,
        _shared: &mut CnCtx,
        event: &CreditNoteEvent,
        _ctx: &mut (),
    ) -> statig::Response<Self> {
        // Each leaf consults the transition table. Illegal events return
        // `Super` (which, given the empty superstate enum, statig will treat
        // as "unhandled"). The wrapping [`StatigAdapter::apply_event`] does
        // its own pre-check so illegal events never actually reach this
        // dispatcher under normal use — these arms exist for completeness.
        let domain_state = self.to_domain();
        match next_state(domain_state, event) {
            Ok(next) => statig::Response::Transition(StatigState::from_domain(next)),
            Err(_) => statig::Response::Super,
        }
    }

    fn call_entry_action(&mut self, _shared: &mut CnCtx, _ctx: &mut ()) {
        // No-op at v1. T-0014 (`hop-top-inv-bus`) hooks `.entered` emission here.
    }

    fn call_exit_action(&mut self, _shared: &mut CnCtx, _ctx: &mut ()) {
        // No-op at v1.
    }

    fn superstate(&mut self) -> Option<StatigSuperstate> {
        None
    }
}

impl blocking::Superstate<CnCtx> for StatigSuperstate {
    fn call_handler(
        &mut self,
        _shared: &mut CnCtx,
        _event: &CreditNoteEvent,
        _ctx: &mut (),
    ) -> statig::Response<StatigState> {
        // No superstate variants ship — this is structurally unreachable.
        match *self {}
    }

    fn call_entry_action(&mut self, _shared: &mut CnCtx, _ctx: &mut ()) {
        match *self {}
    }

    fn call_exit_action(&mut self, _shared: &mut CnCtx, _ctx: &mut ()) {
        match *self {}
    }

    fn superstate(&mut self) -> Option<StatigSuperstate> {
        match *self {}
    }
}

/// The shipped credit-note FSM adapter.
///
/// Holds an inner statig `InitializedStateMachine` plus a shadow
/// [`CreditNoteState`] that tracks the canonical (table-verified) domain
/// state. Mirrors the invoice [`crate::state::adapter_statig::StatigAdapter`]:
/// the shadow is the source of truth, the inner machine is driven whenever
/// its state matches the shadow.
pub struct StatigAdapter {
    inner: blocking::InitializedStateMachine<CnCtx>,
    /// Canonical state, always consulted via [`Self::current_state`].
    shadow: CreditNoteState,
}

impl std::fmt::Debug for StatigAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreditNoteStatigAdapter")
            .field("state", &self.shadow)
            .field("statig_state", &self.inner.state())
            .finish()
    }
}

impl StatigAdapter {
    /// Seed the adapter with a starting state. Credit notes begin in `Draft`;
    /// rehydrated credit notes use whatever was persisted.
    pub fn new(initial: CreditNoteState) -> Self {
        let inner = CnCtx.uninitialized_state_machine().init();
        Self {
            inner,
            shadow: initial,
        }
    }

    /// Borrow the inner statig leaf state (read-only). Useful for tooling
    /// that wants to observe statig's view directly.
    pub fn statig_state(&self) -> StatigState {
        *self.inner.state()
    }

    /// Current canonical domain state.
    pub fn current_state(&self) -> CreditNoteState {
        self.shadow
    }

    /// Compute the post-event state without mutating.
    pub fn dry_run_event(&self, e: &CreditNoteEvent) -> Result<CreditNoteState, TransitionError> {
        next_state(self.shadow, e)
    }

    /// Apply an event, mutating the machine on success.
    pub fn apply_event(&mut self, e: &CreditNoteEvent) -> Result<CreditNoteState, TransitionError> {
        // Canonical verdict from the table. Illegal events bail before we
        // touch the statig machine.
        let next = next_state(self.shadow, e)?;
        // Drive statig if its current state matches the shadow.
        if self.inner.state().to_domain() == self.shadow {
            self.inner.handle(e);
        }
        self.shadow = next;
        Ok(next)
    }
}

// =============================================================================
// Bus event payloads
// =============================================================================

/// Topic name for the pre-transition (veto-able) mechanic event.
pub const TOPIC_PROPOSED: &str = "inv.billing.creditnote.proposed";

/// Topic name for the post-transition (informational) mechanic event.
pub const TOPIC_TRANSITIONED: &str = "inv.billing.creditnote.transitioned";

/// Topic name for the post-entry (informational) mechanic event.
pub const TOPIC_ENTERED: &str = "inv.billing.creditnote.entered";

/// Payload for `inv.billing.creditnote.proposed`.
///
/// Emitted by the commands layer BEFORE the FSM mutates. Subscribers may
/// return an error to veto the transition (matches kit `core/stage`
/// semantics).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreditNoteProposed {
    /// Credit note the transition would affect.
    pub credit_note_id: CreditNoteId,
    /// State the credit note is currently in.
    pub from: CreditNoteState,
    /// State the credit note would move to if not vetoed.
    pub to: CreditNoteState,
    /// The triggering event.
    pub event: CreditNoteEvent,
    /// Who (or what) triggered the proposal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Channel the request arrived on.
    pub channel: HistoryChannel,
    /// Wall-clock at the moment the proposal was constructed.
    pub proposed_at: DateTime<Utc>,
}

impl CreditNoteProposed {
    /// Construct from the parts the commands layer already has.
    pub fn new(
        credit_note_id: CreditNoteId,
        from: CreditNoteState,
        to: CreditNoteState,
        event: CreditNoteEvent,
        actor: Option<String>,
        channel: HistoryChannel,
        proposed_at: DateTime<Utc>,
    ) -> Self {
        Self {
            credit_note_id,
            from,
            to,
            event,
            actor,
            channel,
            proposed_at,
        }
    }

    /// Short tag for the event (matches [`CreditNoteEvent::tag`]).
    pub fn event_tag(&self) -> &'static str {
        self.event.tag()
    }

    /// The topic this payload publishes to.
    pub const TOPIC: &'static str = TOPIC_PROPOSED;
}

/// Payload for `inv.billing.creditnote.transitioned`.
///
/// Emitted by the commands layer AFTER the FSM mutation has been committed.
/// Not veto-able. Carries both endpoints so consumers don't need to maintain
/// a cache to render "X went from A to B".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreditNoteTransitioned {
    /// Credit note that moved.
    pub credit_note_id: CreditNoteId,
    /// State before the transition.
    pub from: CreditNoteState,
    /// State after the transition.
    pub to: CreditNoteState,
    /// The triggering event.
    pub event: CreditNoteEvent,
    /// Who (or what) triggered the transition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Channel the request arrived on.
    pub channel: HistoryChannel,
    /// Wall-clock at commit time.
    pub occurred_at: DateTime<Utc>,
}

impl CreditNoteTransitioned {
    /// Construct from the parts the commands layer already has.
    pub fn new(
        credit_note_id: CreditNoteId,
        from: CreditNoteState,
        to: CreditNoteState,
        event: CreditNoteEvent,
        actor: Option<String>,
        channel: HistoryChannel,
        occurred_at: DateTime<Utc>,
    ) -> Self {
        Self {
            credit_note_id,
            from,
            to,
            event,
            actor,
            channel,
            occurred_at,
        }
    }

    /// Short tag for the event (matches [`CreditNoteEvent::tag`]).
    pub fn event_tag(&self) -> &'static str {
        self.event.tag()
    }

    /// The topic this payload publishes to.
    pub const TOPIC: &'static str = TOPIC_TRANSITIONED;
}

/// Payload for `inv.billing.creditnote.entered`.
///
/// Emitted right after `.transitioned`. Carries only the new state so a
/// subscriber listening for "any time a credit note enters `Issued`" can
/// match on the topic + payload without inspecting the prior state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CreditNoteEntered {
    /// Credit note that entered the state.
    pub credit_note_id: CreditNoteId,
    /// State that was just entered.
    pub state: CreditNoteState,
    /// Channel the request arrived on.
    pub channel: HistoryChannel,
    /// Who (or what) triggered the entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Wall-clock at entry time.
    pub entered_at: DateTime<Utc>,
}

impl CreditNoteEntered {
    /// Construct from the parts the commands layer already has.
    pub fn new(
        credit_note_id: CreditNoteId,
        state: CreditNoteState,
        channel: HistoryChannel,
        actor: Option<String>,
        entered_at: DateTime<Utc>,
    ) -> Self {
        Self {
            credit_note_id,
            state,
            channel,
            actor,
            entered_at,
        }
    }

    /// The topic this payload publishes to.
    pub const TOPIC: &'static str = TOPIC_ENTERED;
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- transition table -------------------------------------------------

    #[test]
    fn draft_issue_goes_to_issued() {
        assert_eq!(
            next_state(CreditNoteState::Draft, &CreditNoteEvent::Issue).unwrap(),
            CreditNoteState::Issued
        );
    }

    #[test]
    fn issued_is_terminal() {
        let err = next_state(CreditNoteState::Issued, &CreditNoteEvent::Issue).unwrap_err();
        assert!(
            matches!(err, TransitionError::Illegal { event: "issue", .. }),
            "issued must reject issue"
        );
    }

    #[test]
    fn event_tag_stable() {
        assert_eq!(CreditNoteEvent::Issue.tag(), "issue");
    }

    #[test]
    fn event_serde_round_trip() {
        let ev = CreditNoteEvent::Issue;
        let s = serde_json::to_string(&ev).unwrap();
        let back: CreditNoteEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);
    }

    // ---- bare facade ------------------------------------------------------

    #[test]
    fn bare_machine_walks_happy_path() {
        let mut m = CreditNoteAdapter::new(CreditNoteState::Draft);
        assert_eq!(m.current_state(), CreditNoteState::Draft);
        assert_eq!(
            m.apply_event(&CreditNoteEvent::Issue).unwrap(),
            CreditNoteState::Issued
        );
        assert_eq!(m.current_state(), CreditNoteState::Issued);
    }

    #[test]
    fn bare_machine_rejects_issue_on_issued() {
        let mut m = CreditNoteAdapter::new(CreditNoteState::Issued);
        let err = m.apply_event(&CreditNoteEvent::Issue).unwrap_err();
        assert!(matches!(
            err,
            TransitionError::Illegal { event: "issue", .. }
        ));
        // State must not advance on rejection.
        assert_eq!(m.current_state(), CreditNoteState::Issued);
    }

    #[test]
    fn bare_propose_does_not_mutate() {
        let m = CreditNoteAdapter::new(CreditNoteState::Draft);
        assert_eq!(
            m.propose(&CreditNoteEvent::Issue).unwrap(),
            CreditNoteState::Issued
        );
        // Still Draft after a non-mutating propose.
        assert_eq!(m.current_state(), CreditNoteState::Draft);
    }

    #[test]
    fn bare_propose_surfaces_illegal_as_veto_error() {
        let m = CreditNoteAdapter::new(CreditNoteState::Issued);
        let err = m.propose(&CreditNoteEvent::Issue).unwrap_err();
        match err {
            VetoError::Vetoed(msg) => {
                assert!(msg.contains("illegal transition"), "got: {msg}");
                assert!(msg.contains("issue"), "got: {msg}");
            }
            other => panic!("expected Vetoed, got {other:?}"),
        }
    }

    #[test]
    fn bare_dry_run_event_does_not_mutate() {
        let m = CreditNoteAdapter::new(CreditNoteState::Draft);
        assert_eq!(
            m.dry_run_event(&CreditNoteEvent::Issue).unwrap(),
            CreditNoteState::Issued
        );
        assert_eq!(m.current_state(), CreditNoteState::Draft);
    }

    // ---- statig adapter ---------------------------------------------------

    #[test]
    fn state_translation_is_total_round_trip() {
        for s in [CreditNoteState::Draft, CreditNoteState::Issued] {
            assert_eq!(StatigState::from_domain(s).to_domain(), s);
        }
    }

    #[test]
    fn statig_adapter_starts_in_draft_by_default() {
        let a = StatigAdapter::new(CreditNoteState::Draft);
        assert_eq!(a.current_state(), CreditNoteState::Draft);
    }

    #[test]
    fn statig_adapter_can_seed_non_default_state() {
        let a = StatigAdapter::new(CreditNoteState::Issued);
        assert_eq!(a.current_state(), CreditNoteState::Issued);
    }

    #[test]
    fn statig_adapter_walks_happy_path() {
        let mut a = StatigAdapter::new(CreditNoteState::Draft);
        assert_eq!(
            a.apply_event(&CreditNoteEvent::Issue).unwrap(),
            CreditNoteState::Issued
        );
        assert_eq!(a.current_state(), CreditNoteState::Issued);
    }

    #[test]
    fn statig_adapter_rejects_issue_on_issued() {
        let mut a = StatigAdapter::new(CreditNoteState::Issued);
        let err = a.apply_event(&CreditNoteEvent::Issue).unwrap_err();
        assert!(matches!(
            err,
            TransitionError::Illegal { event: "issue", .. }
        ));
        // State must not advance on rejection.
        assert_eq!(a.current_state(), CreditNoteState::Issued);
    }

    #[test]
    fn statig_adapter_dry_run_does_not_mutate() {
        let a = StatigAdapter::new(CreditNoteState::Draft);
        assert_eq!(
            a.dry_run_event(&CreditNoteEvent::Issue).unwrap(),
            CreditNoteState::Issued
        );
        assert_eq!(a.current_state(), CreditNoteState::Draft);
    }

    // ---- parity -----------------------------------------------------------

    /// A sequence of events must take both the bare machine and the statig
    /// adapter through the exact same domain-state trace.
    #[test]
    fn parity_with_bare_machine_happy_path() {
        let trace = run_parity(CreditNoteState::Draft, &[CreditNoteEvent::Issue]);
        assert_eq!(trace, vec![CreditNoteState::Issued]);
    }

    #[test]
    fn parity_illegal_events_match_on_both_machines() {
        // Single (initial, event) tuple at v1; kept as bindings so adding
        // more illegal pairs later is a one-line list extension. Clippy
        // dislikes a `for` over a singleton, so destructure directly.
        let (initial, ev) = (CreditNoteState::Issued, CreditNoteEvent::Issue);
        let mut bare = CreditNoteAdapter::new(initial);
        let mut sta = StatigAdapter::new(initial);
        let bare_err = bare.apply_event(&ev).unwrap_err();
        let stat_err = sta.apply_event(&ev).unwrap_err();
        assert_eq!(
            bare_err, stat_err,
            "bare/statig should reject {ev:?} identically from {initial:?}"
        );
        // Neither machine advanced.
        assert_eq!(bare.current_state(), initial);
        assert_eq!(sta.current_state(), initial);
    }

    /// Runs the same event sequence on both machines, asserts each step
    /// agrees, and returns the resulting trace for caller-side assertions.
    fn run_parity(initial: CreditNoteState, events: &[CreditNoteEvent]) -> Vec<CreditNoteState> {
        let mut bare = CreditNoteAdapter::new(initial);
        let mut sta = StatigAdapter::new(initial);
        let mut trace = Vec::new();
        for ev in events {
            let bare_next = bare.apply_event(ev);
            let stat_next = sta.apply_event(ev);
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

    // ---- bus payloads -----------------------------------------------------

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    #[test]
    fn topic_constants_match_design_spec() {
        assert_eq!(TOPIC_PROPOSED, "inv.billing.creditnote.proposed");
        assert_eq!(TOPIC_TRANSITIONED, "inv.billing.creditnote.transitioned");
        assert_eq!(TOPIC_ENTERED, "inv.billing.creditnote.entered");
        // Per-struct mirrors must match the module constants.
        assert_eq!(CreditNoteProposed::TOPIC, TOPIC_PROPOSED);
        assert_eq!(CreditNoteTransitioned::TOPIC, TOPIC_TRANSITIONED);
        assert_eq!(CreditNoteEntered::TOPIC, TOPIC_ENTERED);
    }

    #[test]
    fn proposed_round_trips() {
        let p = CreditNoteProposed::new(
            CreditNoteId::new(),
            CreditNoteState::Draft,
            CreditNoteState::Issued,
            CreditNoteEvent::Issue,
            Some("jad".into()),
            HistoryChannel::Cli,
            now(),
        );
        let json = serde_json::to_string(&p).unwrap();
        let back: CreditNoteProposed = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
        assert_eq!(p.event_tag(), "issue");
    }

    #[test]
    fn transitioned_round_trips() {
        let t = CreditNoteTransitioned::new(
            CreditNoteId::new(),
            CreditNoteState::Draft,
            CreditNoteState::Issued,
            CreditNoteEvent::Issue,
            None,
            HistoryChannel::Bus,
            now(),
        );
        let json = serde_json::to_string(&t).unwrap();
        let back: CreditNoteTransitioned = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
        assert_eq!(t.event_tag(), "issue");
    }

    #[test]
    fn entered_round_trips() {
        let e = CreditNoteEntered::new(
            CreditNoteId::new(),
            CreditNoteState::Issued,
            HistoryChannel::Api,
            Some("agent-7".into()),
            now(),
        );
        let json = serde_json::to_string(&e).unwrap();
        let back: CreditNoteEntered = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
}
