# Story saas-biller-de-01: Webhook delivery records the realised URL in history

**Persona**: [SaaS biller (Berlin, fin+inv as a service)](../../personas/saas-biller-de.md)

## Story

As a SaaS biller, I want my product app to receive a signed webhook POST when
an invoice transitions to `sent`, and have the realised destination URL
recorded in `invoice_state_history` so I can audit which endpoint was hit
on which transition.

## Acceptance criteria

**Given** `inv server` is running on `127.0.0.1:7400`, a static bearer token
`"prod-token"` is configured, and an invoice in `state = "issued"` at
`invoice_01...`
**When** the product app calls
```sh
curl -X POST http://127.0.0.1:7400/v1/invoices/invoice_01.../send \
  -H "Authorization: Bearer prod-token" \
  -H "Content-Type: application/json" \
  -d '{"destination_uri": "webhook://app.example.com/billing/inv-hook"}'
```
**Then** the response is `200 OK` with the command's result body, the
invoice transitions `issued → sent`, an outbound POST hits
`https://app.example.com/billing/inv-hook` carrying
`X-Inv-Signature: sha256=<hex>` (HMAC-SHA256 over the body, keyed by
`webhook_signing_key` per
[reference/api.md#webhook-signature-outbound](../../reference/api.md#webhook-signature-outbound)),
and the body includes `{ invoice, pdf_url, signature }`.

**Given** the POST completed
**When** the product app queries `invoice_state_history` (directly or via
SQL) for the row matching this send
**Then** the row has `to_state = "sent"`, `channel = "api"`, `event =
"send"`, and `metadata` contains the realised destination URI
(`webhook://app.example.com/billing/inv-hook`) so that auditors can prove
which URL was hit on which transition.

**Given** the webhook target returns `5xx`
**When** the same send is retried
**Then** the response surfaces as HTTP `502 webhook-dispatch-failed`
problem-detail (per [reference/api.md#error-responses-rfc-9457](../../reference/api.md#error-responses-rfc-9457)),
the FSM transition that already landed is **not** rolled back (`state =
"sent"`), and the product app is responsible for its own retry.

## Surfaces touched

- HTTP API — `POST /v1/invoices/{id}/send` with `webhook://` URI.
- Store — `invoices`, `invoice_state_history` (with realised destination URI
  in `metadata`).
- Outbound HTTP — POST with `X-Inv-Signature` header.

## Out of scope for this story

- Auto-retry on `5xx` — the product app retries; `inv` does not have a
  retry queue at v1.
- Verifying the webhook signature on the receiver side (downstream concern;
  documented at [reference/api.md#webhook-signature-outbound](../../reference/api.md#webhook-signature-outbound)).
- Resends triggered by re-running the same `/send` — covered by
  [saas-biller-de-02](02-idempotent-resend.md).

## See also

- [reference/api.md](../../reference/api.md) — full route + error table.
- [contracts/invoice-state-fsm.md](../../contracts/invoice-state-fsm.md) — `Issued → Sent` transition + re-send semantics.
