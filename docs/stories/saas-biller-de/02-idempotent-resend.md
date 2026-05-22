# Story saas-biller-de-02: Idempotent re-send does not double-publish on the bus

**Persona**: [SaaS biller (Berlin, fin+inv as a service)](../../personas/saas-biller-de.md)

## Story

As a SaaS biller whose product app may retry billing operations on transient
failures, I want `POST /v1/invoices/{id}/send` calls carrying the same
`idempotency_key` to return the original result without re-emitting bus
events, so that downstream subscribers (and `fin`'s consumer) don't see
duplicate `.sent` events.

## Acceptance criteria

**Given** `inv server` is running, an invoice in `state = "issued"` at
`invoice_01...`, and an `idempotency_key = "send-2026-06-acme-1"`
**When** the product app calls
```sh
curl -X POST http://127.0.0.1:7400/v1/invoices/invoice_01.../send \
  -H "Authorization: Bearer prod-token" \
  -H "Content-Type: application/json" \
  -d '{
    "destination_uri": "webhook://app.example.com/billing/inv-hook",
    "idempotency_key": "send-2026-06-acme-1"
  }'
```
**Then** the invoice transitions `issued → sent`, an
`inv.billing.invoice.sent` payload is queued in the outbox, and the response
includes the resulting state.

**Given** the same call shape is repeated (same `idempotency_key`, same
body)
**When** the second call lands
**Then** the response is identical to the first (same `invoice_id`, same
resulting state, `idempotency_replay: true` in the response payload per
[user/troubleshooting.md#idempotency-replay-returns-the-same-output-unexpectedly](../../user/troubleshooting.md#idempotency-replay-returns-the-same-output-unexpectedly)),
no new `invoice_state_history` row is inserted, and **no second
`inv.billing.invoice.sent`** is queued.

**Given** the same `idempotency_key` is reused with a **different** body
(e.g. different `destination_uri`)
**When** the call lands
**Then** the response is HTTP `409 idempotency-conflict` problem-detail
(per [reference/api.md#error-responses-rfc-9457](../../reference/api.md#error-responses-rfc-9457)),
no state change, no bus emission.

## Surfaces touched

- HTTP API — `POST /v1/invoices/{id}/send` with `idempotency_key`.
- Commands — `idempotency_key` dedup check in `inv-commands` before
  mutation (design [§3.5](../../architecture/design-spec.md#35-cross-cutting-concerns)).
- Store — `invoices.idempotency_key UNIQUE`; `invoice_state_history`
  (no new row on replay).
- Bus — outbox NOT touched on replay.

## Out of scope for this story

- Bus-inbox dedup for inbound `fin.billing.*` events (different mechanism;
  covered by [saas-biller-de-03](03-fin-stripe-payment-paid.md) implicitly).
- Idempotency across distinct invoices — keys are per-command-per-row, not
  global. Reusing a key across two intentionally different drafts is a bug,
  not a feature; see [user/troubleshooting.md](../../user/troubleshooting.md#idempotency-replay-returns-the-same-output-unexpectedly).

## See also

- [reference/event-bus.md#idempotency--ordering-design-44](../../reference/event-bus.md#idempotency--ordering-design-44).
- [architecture/design-spec.md §3.5](../../architecture/design-spec.md#35-cross-cutting-concerns) — caller idempotency invariant.
- [saas-biller-de-01](01-webhook-send-history.md) — the underlying send flow.
