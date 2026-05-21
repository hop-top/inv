# Credit-note state FSM

Authored from [`crates/core/src/state/creditnote.rs`](../../crates/core/src/state/creditnote.rs).

The credit-note FSM is a degenerate two-state, one-event machine: `Draft →
Issued`. Issued is terminal.

## States

```rust
enum CreditNoteState {
    Draft,
    Issued,
}
```

## Events

```rust
enum CreditNoteEvent {
    Issue,    // moves Draft -> Issued
}
```

The enum is open-shaped (rather than a unit struct) so future verbs (`Void`,
`Cancel`) can be added without rippling through callers.

## Diagram

```text
    ┌─────────┐    Issue
    │  Draft  │ ────────────► ┌──────────┐
    └─────────┘               │  Issued  │  terminal — assigns CN-YYYY-NNNN
                              └──────────┘
```

## Transition table

| From | Event | To | Notes |
|---|---|---|---|
| `Draft` | `Issue` | `Issued` | Assigns `number = "CN-YYYY-NNNN"`. |
| `Issued` | `Issue` | (illegal) | Terminal. |
| `Issued` | (any) | (illegal) | Terminal. |

`Issued` is **terminal** — every event is illegal. Reach `Draft` again only
by creating a fresh credit note (`create_credit_note`).

## Refund-driven drafts

When `inv` consumes `fin.billing.payment.refunded`, it automatically creates
a `Draft` credit note against the referenced invoice. The operator (or an
agent) must explicitly call `issue_credit_note` to transition to `Issued`.

Rationale: refunds may need approval, attribution, or text adjustments before
they're customer-visible. Auto-issuing would skip the review step.

## Source — exhaustive match

[`next_state`](../../crates/core/src/state/creditnote.rs) is one match over
`(CreditNoteState, CreditNoteEvent)`. Rust's pattern-exhaustiveness check
enforces compile-time coverage — adding a state or event fails the build
until the table covers the new pair.

## Mechanic events

Credit notes emit the same three-event triplet as invoices:

| Topic | Phase | Veto-able |
|---|---|---|
| `inv.billing.creditnote.proposed` | sync (pre-transition) | yes |
| `inv.billing.creditnote.transitioned` | post (after commit) | no |
| `inv.billing.creditnote.entered` | post (after commit) | no |

Payload shapes mirror the invoice variants — see
[bus-payload-schemas.md](bus-payload-schemas.md).

## See also

- [Invoice FSM](invoice-state-fsm.md).
- Design spec [§3.4](../architecture/design-spec.md).
