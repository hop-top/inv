# Story saas-biller-de-03: Stripe payment via fin auto-advances invoice to paid

**Persona**: [SaaS biller (Berlin, fin+inv as a service)](../../personas/saas-biller-de.md)

## Story

As a SaaS biller, I want a Stripe `payment_intent.succeeded` webhook that
reaches `fin` to ultimately advance the matching `inv` invoice to `paid`
without the product app calling `POST /v1/invoices/{id}/pay` itself, so
that money movement and invoice state stay consistent through the bus.

## Acceptance criteria

**Given** `inv server` is running with the bus consumer subscribed to
`fin.billing.payment.received` (or a remapped topic via `[bus.remap]`), an
invoice in `state = "sent"` at `invoice_01...` with `total = "1667.14"`,
and `fin` has converted a Stripe webhook into a posted payment
**When** `fin` publishes
```json
{
  "topic": "fin.billing.payment.received",
  "event_id": "fin-evt-abc-1",
  "payload": {
    "invoice_ref":     "inv://invoice/invoice_01...",
    "amount":          "1667.14",
    "received_at":     "2026-06-15T10:00:00Z",
    "idempotency_key": "fin-payment-stripe-pi_abc"
  }
}
```
**Then** `bus_inbox` records the inbound `event_id`, the consumer calls the
`mark_paid` command, the invoice transitions `sent → paid` (cumulative ≥
total), and `inv.billing.invoice.paid` is emitted with `cumulative_paid =
"1667.14"` and `actor = "fin"`, `channel = "bus"`.

**Given** the same `event_id` is re-delivered (bus at-least-once semantics)
**When** the consumer processes it
**Then** `bus_inbox` returns "already processed", no command is invoked,
no state change, no duplicate `inv.billing.invoice.paid` emission.

**Given** the payment amount is partial (e.g. `"500.00"` against a `total =
"1667.14"`)
**When** the event is processed
**Then** the invoice transitions `sent → partially_paid`,
`inv.billing.invoice.partially_paid` is emitted with `cumulative_paid =
"500.00"` and `total = "1667.14"`, and a subsequent
`fin.billing.payment.received` for the remaining `"1167.14"` (with a fresh
`event_id`) advances `partially_paid → paid`.

## Surfaces touched

- Bus consumer — `fin.billing.payment.received` → `mark_paid` command.
- Store — `invoices` (state, amount_paid), `invoice_state_history`,
  `bus_inbox` (event_id dedup).
- Bus emission — `inv.billing.invoice.paid` or
  `inv.billing.invoice.partially_paid` via outbox relay.

## Out of scope for this story

- `fin`'s Stripe webhook → posting pipeline (that's `fin`'s concern).
- Refund-driven credit notes via `fin.billing.payment.refunded` (different
  flow; see [contracts/creditnote-state-fsm.md](../../contracts/creditnote-state-fsm.md)).
- Reverse-charge VAT for EU B2B — explicitly out of scope at v1; the
  amount in the inbound event is treated as the realised payment regardless
  of underlying tax treatment.

## See also

- [reference/event-bus.md#consumed-topics](../../reference/event-bus.md#consumed-topics) — every inbound topic + payload.
- [contracts/bus-payload-schemas.md#finbillingpaymentreceived](../../contracts/bus-payload-schemas.md) — `fin.billing.payment.received` payload shape.
- [contracts/invoice-state-fsm.md](../../contracts/invoice-state-fsm.md) — `Sent → Paid` and `Sent → PartiallyPaid` transitions.
