# Story freelancer-qc-03: Reminder ladder for an overdue invoice

**Persona**: [Freelancer (QC, fin+inv local)](../../personas/freelancer-qc.md)

## Story

As a QC freelancer, I want to schedule a sequence of reminders against an
invoice that's been sent but not yet paid, and have the overdue ticker flag
the invoice once its due date passes, so that I can nudge clients without
re-handling each invoice.

## Acceptance criteria

**Given** an invoice in `state = "sent"` with `due_at =
"2026-06-30T00:00:00Z"`
**When** I run
```sh
inv reminder schedule --invoice invoice_01... \
  --at 2026-07-07T09:00:00Z --channel link
inv reminder schedule --invoice invoice_01... \
  --at 2026-07-14T09:00:00Z --channel link
inv reminder schedule --invoice invoice_01... \
  --at 2026-07-28T09:00:00Z --channel link
```
**Then** three reminder rows are persisted in `state = "scheduled"`, each
with `scheduled_at` in the future, and `inv.billing.reminder.scheduled` is
emitted for each.

**Given** today is `2026-07-07T09:01:00Z` (past the first reminder's
`scheduled_at`) and `inv server` is running
**When** the reminders ticker fires
**Then** the first reminder transitions `scheduled → sent`, the invoice
re-sends via the configured `link` channel (FSM stays in `sent` — the
self-edge `Sent → Sent` documented in
[contracts/invoice-state-fsm.md](../../contracts/invoice-state-fsm.md)),
and `inv.billing.reminder.sent` is emitted. The invoice's FSM `transitioned`
/ `entered` events are **not** emitted (the FSM didn't move).

**Given** today is `2026-07-01T00:00:00Z` (one day past `due_at`)
**When** the overdue ticker fires (`inv tick overdue` or the hourly
ticker in `inv server`)
**Then** `inv.billing.invoice.overdue` is emitted with `due_at` and the
overdue ticker's actor (`inv.overdue.ticker`), `invoices.state` stays
`sent` (overdue is a flag, not a state), and the invoice surfaces in
`inv invoice list --state sent` filtered by due-date ≤ today via shell.

## Surfaces touched

- CLI — `inv reminder schedule`, `inv tick overdue`, `inv invoice list`.
- Tickers — reminders (5 min default), overdue (1 h default).
- Store — `reminders`, `invoice_state_history` (reminder rows record their
  own transitions; the invoice's history does NOT gain a row from the
  reminder self-edge).
- Bus — `inv.billing.reminder.scheduled`, `inv.billing.reminder.sent`,
  `inv.billing.invoice.overdue`.

## Out of scope for this story

- Marking the invoice paid (covered by [freelancer-qc-01](01-draft-link-fin-paid.md)).
- SMTP / SMS delivery of the reminder body — not shipped at v1. The
  `--channel link` mints a fresh signed-link URL on each reminder send; the
  freelancer pastes it manually.
- Reminder cancellation after a payment lands — `inv reminder cancel
  <id>` exists but is not exercised in this story.

## See also

- [reference/cli.md#inv-reminder](../../reference/cli.md#inv-reminder) — flag reference.
- [contracts/invoice-state-fsm.md](../../contracts/invoice-state-fsm.md) — the `Sent → Sent` Remind self-edge.
- [contracts/bus-payload-schemas.md](../../contracts/bus-payload-schemas.md) — reminder + overdue payload shapes.
