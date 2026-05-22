# Story freelancer-qc-01: Draft → issue → send via signed link → fin auto-marks paid

**Persona**: [Freelancer (QC, fin+inv local)](../../personas/freelancer-qc.md)

## Story

As a QC freelancer, I want to draft an invoice for a CA client, issue it,
send a signed view-link URL, and have payment auto-apply when `fin` reports
it received, so that I don't manually touch the invoice state after issuing.

## Acceptance criteria

**Given** `inv server` is running (so the bus consumer + outbox relay are
alive) and a customer exists at `customer_01...` with
`address.country = "CA"` and `address.region = "QC"`
**When** I run
```sh
inv invoice draft \
  --customer customer_01... --currency CAD \
  --seller-jurisdiction CA-QC \
  --line "Consulting:10:125.00" \
  --line "Travel:1:200.00" \
  --due 2026-06-30T00:00:00Z \
  --idempotency-key "draft-2026-06-acme-1"
```
**Then** an invoice row is persisted with `state = "draft"`, `subtotal =
"1450.00"`, `tax_total = "0"` (frozen only at issue), and
`invoice_state_history` has a row with `to_state = "draft"` and
`published_at = NULL` until the relay drains.

**Given** the draft above
**When** I run `inv invoice issue invoice_01...`
**Then** state advances to `issued`, the invoice gets a `number` like
`INV-2026-0001`, tax resolves to GST 5% + QST 9.975% on QC → QC (per
[contracts/tax-resolution.md](../../contracts/tax-resolution.md)) for a
`tax_total` of `217.14`, and a PDF is rendered into the blob store.

**Given** the issued invoice
**When** I run `inv invoice send invoice_01... --to link://`
**Then** the command returns a signed URL of the shape documented in
[contracts/signed-link-token.md](../../contracts/signed-link-token.md),
state advances `issued → sent`, and `inv.billing.invoice.sent` is queued in
the outbox.

**Given** `fin` publishes
`fin.billing.payment.received` with `invoice_ref =
"inv://invoice/invoice_01..."` and `amount = "1667.14"`
**When** `inv`'s bus consumer receives the event
**Then** the invoice transitions `sent → paid`,
`inv.billing.invoice.paid` is emitted with `cumulative_paid = "1667.14"`,
and `bus_inbox` records the inbound `event_id` so a replay is a no-op.

## Surfaces touched

- CLI — `inv invoice draft`, `inv invoice issue`, `inv invoice send --to link://`.
- Bus consumer — `fin.billing.payment.received` → `mark_paid` command.
- Store — `invoices`, `invoice_lines`, `invoice_state_history`, `bus_inbox`.
- Render — HTML + PDF at issue.
- Signed-link route — `GET /v/{token}` (not exercised in this story but the
  URL is minted for the customer to fetch).

## Out of scope for this story

- Customer fetching the signed URL (covered by the `.viewed` emission — a
  separate flow).
- Reminder ladder for the period between `sent` and `paid` (covered by
  [freelancer-qc-03](03-reminder-ladder-overdue.md)).
- Partial payments (full payment in this story).

## See also

- [freelancer-qc-02](02-monthly-retainer-schedule.md) — same lifecycle but driven by a recurring schedule.
- [user/how-to-issue.md](../../user/how-to-issue.md) — CLI walkthrough.
- [contracts/invoice-state-fsm.md](../../contracts/invoice-state-fsm.md) — every legal transition this story uses.
