# Invoice state FSM

For anyone who needs to know which transitions are legal and which aren't.
Authored from [`crates/core/src/state/transitions.rs`](../../crates/core/src/state/transitions.rs).

The transition table is the **source of truth**. Both the bare facade
([`BareMachine`](../../crates/core/src/state/machine.rs)) and the statig
adapter ([`StatigAdapter`](../../crates/core/src/state/adapter_statig.rs))
call into the same pure `next_state` function.

## States

```rust
enum InvoiceState {
    Draft,            // mutable: lines, customer, dates
    Issued,           // immutable; numbered; tax frozen
    Sent,             // dispatched via any send channel
    Viewed,           // customer opened the rendered invoice
    PartiallyPaid,    // 0 < amount_paid < total
    Paid,             // amount_paid >= total — terminal
    Voided,           // pre-payment void — terminal
}
```

`overdue` is NOT a state. It's a derived flag (`invoices.nexus_review` /
overdue tracking is via a separate column + the overdue ticker emits
`inv.billing.invoice.overdue` without changing FSM state).

## Events

```rust
enum InvoiceEvent {
    Issue,
    Send,
    Remind,                     // self-edge on Sent; emits reminder.sent, NOT .transitioned
    View,
    Pay { amount_paid, total }, // table classifies as full or partial
    Void,
}
```

## Diagram

```text
                ┌──────────────┐
                │    Draft     │  mutable
                └──────┬───────┘
                       │ Issue
                       ▼
                ┌──────────────┐
        ┌───────┤   Issued     │  numbered + tax frozen
        │       └──────┬───────┘
        │              │ Send
        │              ▼
        │       ┌──────────────┐  ◄── Remind (self-edge; emits reminder.sent)
        │       │    Sent      │
        │       └──────┬───────┘
        │              │ View
        │              ▼
        │       ┌──────────────┐
        │       │   Viewed     │  display only
        │       └──────┬───────┘
        │              │
        │     Pay      │ (partial)            Pay (full)
        │   ┌──────────┼─────────────────────────────────┐
        │   ▼          ▼                                 ▼
   ┌────────────┐  ┌────────────────┐              ┌────────────┐
   │   Voided   │  │ PartiallyPaid  │              │    Paid    │
   └────────────┘  └────────┬───────┘              └────────────┘
        ▲                    │ Pay (remainder)
        │ Void               └──────────────► Paid
        │ (only if amount_paid == 0)
```

## Legal transitions (complete table)

| From | Event | To | Notes |
|---|---|---|---|
| `Draft` | `Issue` | `Issued` | Assigns number, freezes tax, renders PDF. |
| `Issued` | `Send` | `Sent` | Dispatches to URI. |
| `Sent` | `Remind` | `Sent` | Self-edge. Emits `inv.billing.reminder.sent`. **Not** `.transitioned`. |
| `Sent` | `View` | `Viewed` | Via signed-link view. |
| `Issued` | `Pay (full)` | `Paid` | `amount_paid >= total`. |
| `Issued` | `Pay (partial)` | `PartiallyPaid` | `0 < amount_paid < total`. |
| `Sent` | `Pay (full)` | `Paid` | |
| `Sent` | `Pay (partial)` | `PartiallyPaid` | |
| `Viewed` | `Pay (full)` | `Paid` | |
| `Viewed` | `Pay (partial)` | `PartiallyPaid` | |
| `PartiallyPaid` | `Pay (remainder)` | `Paid` | Cumulative ≥ total. |
| `PartiallyPaid` | `Pay (still-partial)` | `PartiallyPaid` | Cumulative still < total. |
| `Issued` | `Void` | `Voided` | Pre-payment only. |
| `Sent` | `Void` | `Voided` | Pre-payment only. |
| `Viewed` | `Void` | `Voided` | Pre-payment only. |
| `PartiallyPaid` | `Void` | `Voided` | **Only if `amount_paid == 0`** at the command boundary. After any payment lands, `Void` is rejected — use a credit note. |

## Illegal transitions

Every `(state, event)` pair NOT in the legal table is **illegal** and surfaces
as `TransitionError::Illegal { state, event }` (serde tag: `illegal`).
Specifically:

| From | Event | Why |
|---|---|---|
| `Draft` | `Send | Pay | View | Remind | Void` | Can't operate on an un-issued invoice. |
| `Issued` | `Issue | View | Remind` | Already issued; view/remind require `Sent`. |
| `Sent` | `Issue` | Already issued. |
| `Viewed` | `Issue | Send` | Already past those states. |
| `PartiallyPaid` | `Issue | Send | View | Remind` | Past those states. |
| `Paid` | (any) | Terminal. |
| `Voided` | (any) | Terminal. |

Additionally, `Pay` with `amount_paid <= 0` rejects with `NonPositivePayment`
(serde tag: `non_positive_payment`).

## Channel uniformity

Every channel (CLI / HTTP API / WS / MCP / bus consumer) routes through
`hop-top-inv-commands`, which calls into the same FSM. The same `(state, event)`
pair is illegal everywhere — no channel-specific exceptions. The error
manifests as:

- CLI: anyhow chain to stderr, exit code 1.
- HTTP API: 409 `fsm-transition` problem-detail.
- WS: `{"error": {"code": "fsm_transition", "message": "..."}}`.
- MCP: `RmcpError` carrying the original `TransitionError`.
- Bus consumer: dispatch records the failure but doesn't ack the inbound
  event (it remains unprocessed in `bus_inbox`).

## Source — exhaustive match

The transition table is one `match (current, event)` in
[`crates/core/src/state/transitions.rs`](../../crates/core/src/state/transitions.rs).
Rust's pattern-exhaustiveness check enforces — at compile time — that every
`(state, event)` pair has a verdict. Adding a new state or event makes the
build fail until the table covers it. **This is the change-detection
mechanism for this contract.**

## See also

- [Credit-note FSM](creditnote-state-fsm.md) — simpler graph (Draft → Issued).
- [Bus payload schemas](bus-payload-schemas.md) — what `.transitioned` and `.entered` carry.
- Design spec [§3.4](../architecture/design-spec.md).
