# Story freelancer-qc-02: Materialise monthly retainer from a recurring schedule

**Persona**: [Freelancer (QC, fin+inv local)](../../personas/freelancer-qc.md)

## Story

As a QC freelancer with a long-running monthly retainer client, I want a
schedule that auto-materialises and auto-issues an invoice on day 1 of every
month so that I don't re-type the same line items each cycle.

## Acceptance criteria

**Given** a customer `customer_01...` exists with QC address
**When** I run
```sh
inv schedule create \
  --customer customer_01... --currency CAD \
  --cadence "monthly@1" --start 2026-06-01 \
  --line "Retainer:1:5000.00" \
  --line "Add-on hours:10:150.00" \
  --auto-issue
```
**Then** a schedule row is persisted in `state = "active"` with
`next_run = "2026-06-01"`, `auto_issue = true`, and
`inv.billing.schedule.created` is queued for emission.

**Given** the schedule above and today's date is `2026-06-01`
**When** the schedules ticker fires (`inv tick schedules`, or the daily
ticker inside `inv server`)
**Then** one new invoice is materialised in `state = "draft"` with
`schedule_id` set to the schedule's id, then immediately transitions
`draft → issued` (because `auto_issue = true`), receives a number like
`INV-2026-0001`, and emits the full triplet + domain events:
```
inv.billing.invoice.drafted
inv.billing.invoice.proposed       (mechanic, pre-issue, veto-able)
inv.billing.invoice.transitioned
inv.billing.invoice.entered
inv.billing.invoice.issued
```
**And** `schedules.next_run` advances to `2026-07-01` and `last_run` to
`2026-06-01`.

**Given** the schedule is no longer needed
**When** I run `inv schedule cancel schedule_01...`
**Then** the schedule transitions to `cancelled` (terminal), the ticker
skips it on subsequent runs, and `inv.billing.schedule.cancelled` is
emitted.

## Surfaces touched

- CLI — `inv schedule create`, `inv tick schedules`, `inv schedule cancel`.
- Store — `schedules`, `invoices` (with `schedule_id` populated),
  `invoice_state_history`.
- Render — HTML + PDF at the auto-issue step.

## Out of scope for this story

- Sending the materialised invoice (covered by [freelancer-qc-01](01-draft-link-fin-paid.md)).
- Proration, plan catalog, usage metering — explicitly out of scope at v1
  per [user/how-to-recurring.md](../../user/how-to-recurring.md#out-of-scope-at-v1).

## See also

- [user/how-to-recurring.md](../../user/how-to-recurring.md) — full schedule walkthrough.
- [contracts/bus-payload-schemas.md](../../contracts/bus-payload-schemas.md) — payload shape for `inv.billing.schedule.created`.
- [reference/event-bus.md](../../reference/event-bus.md) — emission order for the triplet + domain events.
