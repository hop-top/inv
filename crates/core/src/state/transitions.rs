//! Pure transition table for the invoice FSM.
//!
//! Implemented as one exhaustive [`match`] over `(InvoiceState, InvoiceEvent)`.
//! Rust's pattern-exhaustiveness check enforces — at compile time — that every
//! `(state, event)` pair has a verdict. Adding a new variant to either enum
//! fails the build until the table covers it.
//!
//! The table is the **source of truth**. Both the bare facade impl
//! ([`crate::state::machine::BareMachine`]) and the statig adapter
//! ([`crate::state::adapter_statig`]) call into it so they stay in lock-step.
//!
//! Per design spec §3.4:
//!
//! - `Draft + Issue            -> Issued`
//! - `Issued + Send            -> Sent`
//! - `Sent + Remind            -> Sent` (self-edge; reminder.sent, not transitioned)
//! - `Sent + View              -> Viewed`
//! - `Issued|Sent|Viewed + Pay(full)      -> Paid`
//! - `Issued|Sent|Viewed + Pay(partial)   -> PartiallyPaid`
//! - `PartiallyPaid + Pay(remainder)      -> Paid`
//! - `Issued|Sent|Viewed|PartiallyPaid + Void -> Voided`
//! - `Paid`  is terminal — every event is illegal
//! - `Voided` is terminal — every event is illegal

use crate::domain::invoice::InvoiceState;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// FSM-level input event.
///
/// Mirrors the lifecycle verbs in design §3.4. `Pay` carries the amount paid
/// in this single transaction and the invoice's total so the table can
/// classify the payment as partial, full, or a remainder-completing payment
/// without consulting any external state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum InvoiceEvent {
    /// Move `Draft -> Issued`.
    Issue,
    /// Move `Issued -> Sent` (or any sender-initiated send, hierarchically).
    Send,
    /// Self-edge on `Sent`. Emits `reminder.sent`, NOT `.transitioned`.
    Remind,
    /// Move `Sent -> Viewed`.
    View,
    /// Record a payment. The total paid so far (after this payment, including
    /// any previous partials) is compared against `total` to classify as
    /// full, partial, or remainder-completing.
    Pay {
        /// Amount paid in this single transaction.
        amount_paid: Decimal,
        /// Invoice total (after tax) for comparison.
        total: Decimal,
    },
    /// Void the invoice. Legal only pre-payment; never after `Paid`.
    Void,
}

impl InvoiceEvent {
    /// Short string tag used in audit rows and bus payloads.
    /// (Distinct from the topic shape — see `crate::state::events`.)
    pub fn tag(&self) -> &'static str {
        match self {
            InvoiceEvent::Issue => "issue",
            InvoiceEvent::Send => "send",
            InvoiceEvent::Remind => "remind",
            InvoiceEvent::View => "view",
            InvoiceEvent::Pay { .. } => "pay",
            InvoiceEvent::Void => "void",
        }
    }
}

/// Why a `(state, event)` pair was rejected by the transition table.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum TransitionError {
    /// The event has no legal effect from the given state.
    ///
    /// E.g. `Pay` on `Draft`, `Void` on `Paid`, anything on a terminal state.
    #[error("illegal transition: {event} not allowed from {state:?}")]
    Illegal {
        /// Current state at the time of the event.
        state: InvoiceState,
        /// Event tag (see [`InvoiceEvent::tag`]).
        event: &'static str,
    },
    /// `Pay` was issued but `amount_paid` was zero or negative.
    #[error("payment must be positive (got amount_paid={amount_paid})")]
    NonPositivePayment {
        /// The non-positive amount the caller passed in.
        amount_paid: Decimal,
    },
}

/// Verdict of a payment classification.
///
/// Used by both the bare and statig paths so they classify identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentKind {
    /// `amount_paid >= total`.
    Full,
    /// `0 < amount_paid < total`.
    Partial,
}

/// Classify a payment by comparing `amount_paid` to `total`.
///
/// Returns `Err` if `amount_paid <= 0`. The transition table never produces
/// `Voided` or `Paid` for a non-positive payment.
pub fn classify_payment(
    amount_paid: Decimal,
    total: Decimal,
) -> Result<PaymentKind, TransitionError> {
    if amount_paid <= Decimal::ZERO {
        return Err(TransitionError::NonPositivePayment { amount_paid });
    }
    if amount_paid >= total {
        Ok(PaymentKind::Full)
    } else {
        Ok(PaymentKind::Partial)
    }
}

/// Resolve the next state for `(current, event)`.
///
/// This is the pure transition function. It does not mutate anything; it
/// does not emit events; it does not touch the bus. Callers (the facade
/// trait, the statig adapter, the commands layer) compose around it.
///
/// `Sent + Remind` returns `Ok(Sent)` — a self-edge. Callers MUST detect
/// this case (current state == next state on `Remind`) and emit
/// `inv.billing.reminder.sent` rather than `.transitioned` per design §3.4.
pub fn next_state(
    current: InvoiceState,
    event: &InvoiceEvent,
) -> Result<InvoiceState, TransitionError> {
    use InvoiceEvent as E;
    use InvoiceState as S;

    // The exhaustive match. Adding a variant to either enum forces every
    // pair below to be reconsidered — compile-time guarantee.
    match (current, event) {
        // -- draft -----------------------------------------------------------
        (S::Draft, E::Issue) => Ok(S::Issued),
        (S::Draft, E::Send | E::Remind | E::View | E::Pay { .. } | E::Void) => Err(illegal(current, event)),

        // -- issued ----------------------------------------------------------
        (S::Issued, E::Send) => Ok(S::Sent),
        (S::Issued, E::Pay { amount_paid, total }) => pay_from_open(*amount_paid, *total),
        (S::Issued, E::Void) => Ok(S::Voided),
        (S::Issued, E::Issue | E::Remind | E::View) => Err(illegal(current, event)),

        // -- sent ------------------------------------------------------------
        (S::Sent, E::Remind) => Ok(S::Sent), // self-edge; caller emits reminder.sent
        (S::Sent, E::View) => Ok(S::Viewed),
        (S::Sent, E::Pay { amount_paid, total }) => pay_from_open(*amount_paid, *total),
        (S::Sent, E::Void) => Ok(S::Voided),
        (S::Sent, E::Issue | E::Send) => Err(illegal(current, event)),

        // -- viewed ----------------------------------------------------------
        (S::Viewed, E::Pay { amount_paid, total }) => pay_from_open(*amount_paid, *total),
        (S::Viewed, E::Void) => Ok(S::Voided),
        (S::Viewed, E::Issue | E::Send | E::Remind | E::View) => Err(illegal(current, event)),

        // -- partially_paid --------------------------------------------------
        (S::PartiallyPaid, E::Pay { amount_paid, total }) => {
            // A subsequent payment from PartiallyPaid: if it covers the
            // remainder we reach Paid; otherwise we stay PartiallyPaid.
            // Note: callers must pass `amount_paid` as the cumulative total
            // paid (including prior partials) to get the right verdict.
            match classify_payment(*amount_paid, *total)? {
                PaymentKind::Full => Ok(S::Paid),
                PaymentKind::Partial => Ok(S::PartiallyPaid),
            }
        }
        (S::PartiallyPaid, E::Void) => Ok(S::Voided),
        (S::PartiallyPaid, E::Issue | E::Send | E::Remind | E::View) => Err(illegal(current, event)),

        // -- paid (terminal) -------------------------------------------------
        (S::Paid, _) => Err(illegal(current, event)),

        // -- voided (terminal) -----------------------------------------------
        (S::Voided, _) => Err(illegal(current, event)),
    }
}

/// Common payment-from-open-state helper (Issued / Sent / Viewed).
fn pay_from_open(
    amount_paid: Decimal,
    total: Decimal,
) -> Result<InvoiceState, TransitionError> {
    match classify_payment(amount_paid, total)? {
        PaymentKind::Full => Ok(InvoiceState::Paid),
        PaymentKind::Partial => Ok(InvoiceState::PartiallyPaid),
    }
}

#[inline]
fn illegal(state: InvoiceState, event: &InvoiceEvent) -> TransitionError {
    TransitionError::Illegal {
        state,
        event: event.tag(),
    }
}

/// True when a transition is a self-edge that should NOT emit `.transitioned`
/// / `.entered`. Currently the only such case is `Sent + Remind`.
///
/// Callers (the commands layer in T-0011) use this to choose between
/// `.transitioned` / `.entered` and the reminder-specific topic.
pub fn is_self_edge(current: InvoiceState, event: &InvoiceEvent) -> bool {
    matches!((current, event), (InvoiceState::Sent, InvoiceEvent::Remind))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::FromStr;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    fn pay(amount: &str, total: &str) -> InvoiceEvent {
        InvoiceEvent::Pay {
            amount_paid: dec(amount),
            total: dec(total),
        }
    }

    // ---- legal transitions --------------------------------------------------

    #[test]
    fn draft_issue_goes_to_issued() {
        assert_eq!(
            next_state(InvoiceState::Draft, &InvoiceEvent::Issue).unwrap(),
            InvoiceState::Issued
        );
    }

    #[test]
    fn issued_send_goes_to_sent() {
        assert_eq!(
            next_state(InvoiceState::Issued, &InvoiceEvent::Send).unwrap(),
            InvoiceState::Sent
        );
    }

    #[test]
    fn sent_view_goes_to_viewed() {
        assert_eq!(
            next_state(InvoiceState::Sent, &InvoiceEvent::View).unwrap(),
            InvoiceState::Viewed
        );
    }

    #[test]
    fn sent_remind_is_self_edge() {
        assert_eq!(
            next_state(InvoiceState::Sent, &InvoiceEvent::Remind).unwrap(),
            InvoiceState::Sent
        );
        assert!(is_self_edge(InvoiceState::Sent, &InvoiceEvent::Remind));
        assert!(!is_self_edge(InvoiceState::Sent, &InvoiceEvent::View));
    }

    #[test]
    fn issued_full_payment_goes_to_paid() {
        assert_eq!(
            next_state(InvoiceState::Issued, &pay("100.00", "100.00")).unwrap(),
            InvoiceState::Paid
        );
        assert_eq!(
            next_state(InvoiceState::Issued, &pay("150.00", "100.00")).unwrap(),
            InvoiceState::Paid
        );
    }

    #[test]
    fn sent_partial_payment_goes_to_partially_paid() {
        assert_eq!(
            next_state(InvoiceState::Sent, &pay("40.00", "100.00")).unwrap(),
            InvoiceState::PartiallyPaid
        );
    }

    #[test]
    fn viewed_full_payment_goes_to_paid() {
        assert_eq!(
            next_state(InvoiceState::Viewed, &pay("100.00", "100.00")).unwrap(),
            InvoiceState::Paid
        );
    }

    #[test]
    fn viewed_partial_payment_goes_to_partially_paid() {
        assert_eq!(
            next_state(InvoiceState::Viewed, &pay("25.00", "100.00")).unwrap(),
            InvoiceState::PartiallyPaid
        );
    }

    #[test]
    fn partial_payment_remainder_goes_to_paid() {
        // amount_paid here represents cumulative total paid; the table
        // checks `>= total`.
        assert_eq!(
            next_state(InvoiceState::PartiallyPaid, &pay("100.00", "100.00")).unwrap(),
            InvoiceState::Paid
        );
    }

    #[test]
    fn partial_payment_still_partial() {
        assert_eq!(
            next_state(InvoiceState::PartiallyPaid, &pay("80.00", "100.00")).unwrap(),
            InvoiceState::PartiallyPaid
        );
    }

    #[test]
    fn void_legal_from_open_and_partial() {
        for s in [
            InvoiceState::Issued,
            InvoiceState::Sent,
            InvoiceState::Viewed,
            InvoiceState::PartiallyPaid,
        ] {
            assert_eq!(
                next_state(s, &InvoiceEvent::Void).unwrap(),
                InvoiceState::Voided,
                "void should be legal from {s:?}"
            );
        }
    }

    // ---- illegal transitions ------------------------------------------------

    #[test]
    fn pay_on_draft_is_illegal() {
        let err = next_state(InvoiceState::Draft, &pay("50.00", "100.00")).unwrap_err();
        assert!(matches!(err, TransitionError::Illegal { event: "pay", .. }));
    }

    #[test]
    fn void_on_draft_is_illegal() {
        // Draft is mutable; you delete a draft, you don't void it.
        let err = next_state(InvoiceState::Draft, &InvoiceEvent::Void).unwrap_err();
        assert!(matches!(err, TransitionError::Illegal { event: "void", .. }));
    }

    #[test]
    fn void_on_paid_is_illegal() {
        let err = next_state(InvoiceState::Paid, &InvoiceEvent::Void).unwrap_err();
        assert!(matches!(err, TransitionError::Illegal { event: "void", .. }));
    }

    #[test]
    fn paid_is_terminal() {
        for ev in [
            InvoiceEvent::Issue,
            InvoiceEvent::Send,
            InvoiceEvent::Remind,
            InvoiceEvent::View,
            InvoiceEvent::Void,
            pay("10.00", "100.00"),
        ] {
            let err = next_state(InvoiceState::Paid, &ev).unwrap_err();
            assert!(
                matches!(err, TransitionError::Illegal { .. }),
                "paid must reject {ev:?}"
            );
        }
    }

    #[test]
    fn voided_is_terminal() {
        for ev in [
            InvoiceEvent::Issue,
            InvoiceEvent::Send,
            InvoiceEvent::Remind,
            InvoiceEvent::View,
            InvoiceEvent::Void,
            pay("10.00", "100.00"),
        ] {
            let err = next_state(InvoiceState::Voided, &ev).unwrap_err();
            assert!(
                matches!(err, TransitionError::Illegal { .. }),
                "voided must reject {ev:?}"
            );
        }
    }

    #[test]
    fn issue_on_already_issued_is_illegal() {
        let err = next_state(InvoiceState::Issued, &InvoiceEvent::Issue).unwrap_err();
        assert!(matches!(err, TransitionError::Illegal { .. }));
    }

    #[test]
    fn view_on_issued_is_illegal() {
        // Viewed requires Sent (signed-link delivery is the only viewer
        // signal path). Per the FSM diagram, view is reachable only from
        // Sent.
        let err = next_state(InvoiceState::Issued, &InvoiceEvent::View).unwrap_err();
        assert!(matches!(err, TransitionError::Illegal { .. }));
    }

    #[test]
    fn remind_on_issued_is_illegal() {
        // The remind self-edge only exists on Sent.
        let err = next_state(InvoiceState::Issued, &InvoiceEvent::Remind).unwrap_err();
        assert!(matches!(err, TransitionError::Illegal { .. }));
    }

    #[test]
    fn non_positive_payment_rejected() {
        let err = next_state(InvoiceState::Issued, &pay("0", "100.00")).unwrap_err();
        assert!(matches!(err, TransitionError::NonPositivePayment { .. }));
        let err = next_state(InvoiceState::Issued, &pay("-5.00", "100.00")).unwrap_err();
        assert!(matches!(err, TransitionError::NonPositivePayment { .. }));
    }

    #[test]
    fn event_serde_round_trip() {
        let ev = pay("12.34", "100.00");
        let s = serde_json::to_string(&ev).unwrap();
        let back: InvoiceEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(ev, back);
    }

    #[test]
    fn event_tag_strings_stable() {
        assert_eq!(InvoiceEvent::Issue.tag(), "issue");
        assert_eq!(InvoiceEvent::Send.tag(), "send");
        assert_eq!(InvoiceEvent::Remind.tag(), "remind");
        assert_eq!(InvoiceEvent::View.tag(), "view");
        assert_eq!(InvoiceEvent::Void.tag(), "void");
        assert_eq!(pay("1.00", "2.00").tag(), "pay");
    }
}
