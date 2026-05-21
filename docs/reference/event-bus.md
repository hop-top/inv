# Event bus reference

Every topic `inv` emits and consumes. Authored from
[`crates/bus/src/events.rs`](../../crates/bus/src/events.rs),
[`crates/core/src/state/events.rs`](../../crates/core/src/state/events.rs),
[`crates/core/src/state/creditnote.rs`](../../crates/core/src/state/creditnote.rs),
and [`crates/bus/src/consumer.rs`](../../crates/bus/src/consumer.rs).

## Naming convention

Per kit (`kit/go/runtime/bus/event.go`): topics are dot-separated as
**`[Source].[Category].[Object].[Action]`**. Pattern matching follows MQTT —
`*` matches one segment, `#` matches zero or more trailing segments.

`inv` uses `Source = inv` and `Category = billing` across every topic it
emits.

## Domain events (emitted by `inv`)

These describe **what happened in business terms** and are the primary
integration surface for `fin`, dashboards, and downstream tools.

| Topic | When |
|---|---|
| `inv.billing.invoice.drafted` | A new invoice was created in `Draft`. |
| `inv.billing.invoice.issued` | Transitioned `Draft → Issued`. |
| `inv.billing.invoice.sent` | Delivered (any send scheme). |
| `inv.billing.invoice.viewed` | Customer opened the rendered invoice (via signed-link route). |
| `inv.billing.invoice.paid` | Cumulative payment ≥ total. |
| `inv.billing.invoice.partially_paid` | Payment recorded but cumulative < total. |
| `inv.billing.invoice.overdue` | Due date passed, still unpaid. Flag-only — does not change FSM state. |
| `inv.billing.invoice.voided` | Pre-payment void. Terminal. |
| `inv.billing.creditnote.drafted` | Credit note created in `Draft`. |
| `inv.billing.creditnote.issued` | Credit note transitioned `Draft → Issued`. Terminal. |
| `inv.billing.reminder.scheduled` | Reminder enqueued for future dispatch. |
| `inv.billing.reminder.sent` | Reminder dispatched. |
| `inv.billing.reminder.cancelled` | Reminder cancelled before send. |
| `inv.billing.schedule.created` | Recurring schedule created. |
| `inv.billing.schedule.paused` | Schedule paused. |
| `inv.billing.schedule.cancelled` | Schedule cancelled. Terminal. |

## Mechanic events (emitted by `inv`)

These describe **how the FSM moved**. They mirror kit's `core/stage`
semantics for any tooling that wants to observe transitions generically (veto
subscribers, audit replay, observability).

| Topic | Phase | Veto-able | Source |
|---|---|---|---|
| `inv.billing.invoice.proposed` | sync (pre-transition) | yes | commands layer before mutation |
| `inv.billing.invoice.transitioned` | post (after commit) | no | commands layer after commit |
| `inv.billing.invoice.entered` | post (after commit) | no | commands layer after commit |
| `inv.billing.creditnote.proposed` | sync | yes | commands layer before mutation |
| `inv.billing.creditnote.transitioned` | post | no | commands layer after commit |
| `inv.billing.creditnote.entered` | post | no | commands layer after commit |

Both sets are emitted; domain events are emitted **after** the mechanic
events for the same transition. Order for `Draft → Issued`:

```
inv.billing.invoice.proposed       (mechanic, veto-able)
inv.billing.invoice.transitioned   (mechanic)
inv.billing.invoice.entered        (mechanic)
inv.billing.invoice.issued         (domain)
```

## Consumed topics

`inv`'s bus consumer routes inbound `fin.billing.*` events to commands. Topic
names are configurable so deployments can map them onto whatever `fin` (or
any other publisher) ultimately ships — use `[bus.remap]` in `inv.toml`.

| Topic | Payload (minimum) | `inv` action |
|---|---|---|
| `fin.billing.charge.created` | `{ customer_id, currency, lines[], due_date?, idempotency_key? }` | Call `draft_invoice`. |
| `fin.billing.payment.received` | `{ invoice_ref, amount, received_at, idempotency_key? }` | Call `mark_paid`. |
| `fin.billing.payment.refunded` | `{ invoice_ref, amount, reason?, refund_id? }` | Call `create_credit_note`. |

`invoice_ref` accepts either a bare typeid (`invoice_01J...`) or a poly-uri
URI (`inv://invoice/invoice_01J...`).

## Payload shape

All payload structs round-trip through `serde_json`. Money values are quoted
strings (`Decimal` canonical form). Topic constants live in
[`crates/bus/src/events.rs`](../../crates/bus/src/events.rs). Detailed
JSON-schema sketches: [contracts/bus-payload-schemas.md](../contracts/bus-payload-schemas.md).

### Example: `inv.billing.invoice.issued`

```json
{
  "invoice_id": "invoice_01j9zl...",
  "number": "INV-2026-0001",
  "customer_id": "customer_01j9zk...",
  "currency": "CAD",
  "subtotal": "1450.00",
  "tax_total": "217.14",
  "total": "1667.14",
  "nexus_review": false,
  "actor": "jad",
  "channel": "cli"
}
```

### Example: `inv.billing.invoice.proposed` (mechanic)

```json
{
  "invoice_id": "invoice_01j9zl...",
  "from": "Draft",
  "to": "Issued",
  "event": { "kind": "issue" },
  "actor": "jad",
  "channel": "Cli",
  "proposed_at": "2026-05-20T10:00:00Z"
}
```

## Idempotency + ordering (design §4.4)

- Every command takes an optional `idempotency_key`. The first request with
  a given key persists; subsequent requests return the original result.
- Bus events carry `event_id`. The `bus_inbox` table records every received
  event before processing; reprocessing the same `event_id` is a no-op.
- The outbox pattern guarantees **at-least-once** outbound delivery. History
  rows with `published_at IS NULL` are published by the relay then marked.
  Subscribers must tolerate at-least-once.

## Outbox relay

Runs inside `inv server`. Drains `invoice_state_history` +
`credit_note_state_history` rows where `published_at IS NULL`, publishes the
corresponding domain + mechanic events through kit's bus, and marks rows
processed in the same transaction.

Interval: `--outbox-interval-secs 5` (default). The CLI-only / one-shot path
does **not** run the relay; events accumulate in history and are flushed when
a server boots against the same DB.

## See also

- [contracts/bus-payload-schemas.md](../contracts/bus-payload-schemas.md) — JSON-schema sketches.
- [WebSocket reference](ws.md) — subscribe to topics over a long-lived connection.
- Design spec [§4](../architecture/design-spec.md) — full event-bus design.
