//! Bus event shapes for FSM **mechanic** events.
//!
//! Per design §4.2, three topics describe how the invoice FSM moves
//! generically:
//!
//! | Topic | Phase | Veto-able |
//! |---|---|---|
//! | `inv.billing.invoice.proposed` | sync (pre-transition) | yes |
//! | `inv.billing.invoice.transitioned` | post (after commit) | no |
//! | `inv.billing.invoice.entered` | post (after commit) | no |
//!
//! **Domain** events (`drafted`, `issued`, `sent`, `paid`, etc.) describe
//! *what happened in business terms* and live in `hop-top-inv-bus` (T-0014). This
//! module only carries the mechanic-event payloads — the seam kit's
//! `core/stage` provides for generic observers and veto subscribers.
//!
//! No bus wiring lives here. T-0014 (`hop-top-inv-bus`) consumes these structs,
//! attaches metadata (event_id, timestamps, source), and publishes via
//! `hop-top-kit`'s bus.

use crate::domain::ids::InvoiceId;
use crate::domain::invoice::{HistoryChannel, InvoiceState};
use crate::state::transitions::InvoiceEvent;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Topic name for the pre-transition (veto-able) mechanic event.
pub const TOPIC_PROPOSED: &str = "inv.billing.invoice.proposed";

/// Topic name for the post-transition (informational) mechanic event.
pub const TOPIC_TRANSITIONED: &str = "inv.billing.invoice.transitioned";

/// Topic name for the post-entry (informational) mechanic event.
pub const TOPIC_ENTERED: &str = "inv.billing.invoice.entered";

/// Payload for `inv.billing.invoice.proposed`.
///
/// Emitted by the commands layer BEFORE the FSM mutates. Subscribers may
/// return an error to veto the transition (matches kit `core/stage`
/// semantics — see design §3.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InvoiceProposed {
    /// Invoice the transition would affect.
    pub invoice_id: InvoiceId,
    /// State the invoice is currently in.
    pub from: InvoiceState,
    /// State the invoice would move to if not vetoed.
    pub to: InvoiceState,
    /// The triggering event.
    pub event: InvoiceEvent,
    /// Who (or what) triggered the proposal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Channel the request arrived on.
    pub channel: HistoryChannel,
    /// Wall-clock at the moment the proposal was constructed.
    pub proposed_at: DateTime<Utc>,
}

impl InvoiceProposed {
    /// Construct from the parts the commands layer already has.
    pub fn new(
        invoice_id: InvoiceId,
        from: InvoiceState,
        to: InvoiceState,
        event: InvoiceEvent,
        actor: Option<String>,
        channel: HistoryChannel,
        proposed_at: DateTime<Utc>,
    ) -> Self {
        Self {
            invoice_id,
            from,
            to,
            event,
            actor,
            channel,
            proposed_at,
        }
    }

    /// Short tag for the event (matches [`InvoiceEvent::tag`]).
    pub fn event_tag(&self) -> &'static str {
        self.event.tag()
    }

    /// The topic this payload publishes to.
    pub const TOPIC: &'static str = TOPIC_PROPOSED;
}

/// Payload for `inv.billing.invoice.transitioned`.
///
/// Emitted by the commands layer AFTER the FSM mutation has been committed.
/// Not veto-able. Carries both endpoints so consumers don't have to maintain
/// a cache to render "X went from A to B".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InvoiceTransitioned {
    /// Invoice that moved.
    pub invoice_id: InvoiceId,
    /// State before the transition.
    pub from: InvoiceState,
    /// State after the transition.
    pub to: InvoiceState,
    /// The triggering event.
    pub event: InvoiceEvent,
    /// Who (or what) triggered the transition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Channel the request arrived on.
    pub channel: HistoryChannel,
    /// Wall-clock at commit time.
    pub occurred_at: DateTime<Utc>,
}

impl InvoiceTransitioned {
    /// Construct from the parts the commands layer already has.
    pub fn new(
        invoice_id: InvoiceId,
        from: InvoiceState,
        to: InvoiceState,
        event: InvoiceEvent,
        actor: Option<String>,
        channel: HistoryChannel,
        occurred_at: DateTime<Utc>,
    ) -> Self {
        Self {
            invoice_id,
            from,
            to,
            event,
            actor,
            channel,
            occurred_at,
        }
    }

    /// Short tag for the event (matches [`InvoiceEvent::tag`]).
    pub fn event_tag(&self) -> &'static str {
        self.event.tag()
    }

    /// The topic this payload publishes to.
    pub const TOPIC: &'static str = TOPIC_TRANSITIONED;
}

/// Payload for `inv.billing.invoice.entered`.
///
/// Emitted right after `.transitioned`. Carries only the new state so a
/// subscriber listening for "any time an invoice enters `Paid`" can match
/// on the topic + payload without inspecting the prior state.
///
/// kit `core/stage` semantics: `.entered` fires once per leaf state entry,
/// after entry actions, after `.transitioned`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InvoiceEntered {
    /// Invoice that entered the state.
    pub invoice_id: InvoiceId,
    /// State that was just entered.
    pub state: InvoiceState,
    /// Channel the request arrived on.
    pub channel: HistoryChannel,
    /// Who (or what) triggered the entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Wall-clock at entry time.
    pub entered_at: DateTime<Utc>,
}

impl InvoiceEntered {
    /// Construct from the parts the commands layer already has.
    pub fn new(
        invoice_id: InvoiceId,
        state: InvoiceState,
        channel: HistoryChannel,
        actor: Option<String>,
        entered_at: DateTime<Utc>,
    ) -> Self {
        Self {
            invoice_id,
            state,
            channel,
            actor,
            entered_at,
        }
    }

    /// The topic this payload publishes to.
    pub const TOPIC: &'static str = TOPIC_ENTERED;
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;

    fn now() -> DateTime<Utc> {
        // Deterministic-ish; doesn't actually matter for round-trip tests.
        Utc::now()
    }

    #[test]
    fn topic_constants_match_design_spec() {
        assert_eq!(TOPIC_PROPOSED, "inv.billing.invoice.proposed");
        assert_eq!(TOPIC_TRANSITIONED, "inv.billing.invoice.transitioned");
        assert_eq!(TOPIC_ENTERED, "inv.billing.invoice.entered");
        // Per-struct mirrors must match the module constants.
        assert_eq!(InvoiceProposed::TOPIC, TOPIC_PROPOSED);
        assert_eq!(InvoiceTransitioned::TOPIC, TOPIC_TRANSITIONED);
        assert_eq!(InvoiceEntered::TOPIC, TOPIC_ENTERED);
    }

    #[test]
    fn proposed_round_trips() {
        let p = InvoiceProposed::new(
            InvoiceId::new(),
            InvoiceState::Draft,
            InvoiceState::Issued,
            InvoiceEvent::Issue,
            Some("jad".into()),
            HistoryChannel::Cli,
            now(),
        );
        let json = serde_json::to_string(&p).unwrap();
        let back: InvoiceProposed = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
        assert_eq!(p.event_tag(), "issue");
    }

    #[test]
    fn transitioned_round_trips() {
        let t = InvoiceTransitioned::new(
            InvoiceId::new(),
            InvoiceState::Issued,
            InvoiceState::Paid,
            InvoiceEvent::Pay {
                amount_paid: Decimal::new(10000, 2),
                total: Decimal::new(10000, 2),
            },
            None,
            HistoryChannel::Bus,
            now(),
        );
        let json = serde_json::to_string(&t).unwrap();
        let back: InvoiceTransitioned = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
        assert_eq!(t.event_tag(), "pay");
    }

    #[test]
    fn entered_round_trips() {
        let e = InvoiceEntered::new(
            InvoiceId::new(),
            InvoiceState::Voided,
            HistoryChannel::Api,
            Some("agent-7".into()),
            now(),
        );
        let json = serde_json::to_string(&e).unwrap();
        let back: InvoiceEntered = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
}
