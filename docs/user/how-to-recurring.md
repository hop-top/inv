# How to bill on a recurring schedule

For an operator who wants to invoice the same customer on a fixed cadence
(monthly retainer, quarterly licence, annual support contract) without
re-typing line items every cycle.

A **schedule** is a recipe for materialising invoices. The internal
**schedules ticker** runs daily (configurable; see [reference/config.md](../reference/config.md))
and, for every `active` schedule with `next_run <= today`:

1. Materialises a draft invoice with the schedule's template lines.
2. Emits `inv.billing.invoice.drafted` + `inv.billing.schedule.created`-related events.
3. If `auto_issue = true`, runs `issue_invoice` immediately and emits
   `inv.billing.invoice.issued`.
4. Advances `next_run` and `last_run`.

## Create a monthly schedule

```sh
$ inv schedule create \
    --customer customer_01... \
    --currency CAD \
    --cadence "monthly@1" \
    --start 2026-06-01 \
    --line "Retainer:1:5000.00" \
    --line "Add-on hours:10:150.00" \
    --auto-issue
```

Cadence syntax:

- `monthly@<dom>` — every month on the given day-of-month (`monthly@1`,
  `monthly@15`). DOM must be 1–28 to avoid month-end ambiguity.
- `quarterly@<dom>` — every three months on the given day-of-month.
- `yearly@MM-DD` — once per year on the given month-day.

The `--auto-issue` flag opts the schedule into immediate transition from
`draft → issued` on every cycle. Without it, you (or an agent) must manually
call `inv invoice issue <id>` for each materialised draft.

## Pause / resume / cancel

```sh
$ inv schedule pause   schedule_01...    # active → paused; ticker skips it
$ inv schedule cancel  schedule_01...    # → cancelled (terminal)
```

Paused schedules can be resumed by creating a new one (there is no `resume`
verb at v1). Cancellation is terminal; create a fresh schedule if the
customer wants to come back.

## Inspect

```sh
$ inv schedule show  schedule_01...
$ inv schedule list  --customer customer_01...
```

At v1, `schedule list` requires `--customer` — cross-customer listing isn't
plumbed through `ScheduleRepo` yet. (Track this against
[`list_for_customer`](../../crates/store/src/repo/schedule.rs); upstream issue
to relax this restriction is open.)

## Manual ticker (for tests)

If you're testing the materialisation logic without waiting 24h:

```sh
$ inv tick schedules
```

This is the same code path the `inv server` ticker calls on its interval.

## HTTP API equivalents

```sh
$ curl -X POST http://127.0.0.1:7400/v1/schedules \
       -H "Authorization: Bearer dev-token" \
       -H "Content-Type: application/json" \
       -d '{
         "customer_id": "customer_01...",
         "currency": "CAD",
         "cadence": "monthly@1",
         "start_date": "2026-06-01",
         "template_lines": [
           {"description": "Retainer",       "quantity": "1",  "unit_price": "5000.00"},
           {"description": "Add-on hours",   "quantity": "10", "unit_price": "150.00"}
         ],
         "auto_issue": true
       }'

$ curl -X POST http://127.0.0.1:7400/v1/schedules/schedule_01.../pause  -H "Authorization: Bearer dev-token"
$ curl -X POST http://127.0.0.1:7400/v1/schedules/schedule_01.../cancel -H "Authorization: Bearer dev-token"

$ curl -X POST http://127.0.0.1:7400/v1/tick/schedules -H "Authorization: Bearer dev-token"
```

## Materialised invoices carry the schedule id

Every invoice produced by a schedule has its `schedule_id` column populated.
You can filter manually in sql, or follow the audit chain via
`invoice_state_history` rows where `actor` is `"inv.schedule.ticker"`.

## What you'll see on the bus

For each cycle on an `auto_issue` schedule:

```
inv.billing.invoice.drafted              # the new draft
inv.billing.invoice.proposed             # mechanic, pre-issue
inv.billing.invoice.transitioned
inv.billing.invoice.entered
inv.billing.invoice.issued               # domain
```

Schedules themselves also emit lifecycle events:
`inv.billing.schedule.created`, `.paused`, `.cancelled`. Payload shapes:
[contracts/bus-payload-schemas.md](../contracts/bus-payload-schemas.md).

## Out of scope at v1

- **Proration.** Schedules issue the same template lines every cycle.
- **Plan catalog.** No SKU / product / pricing-table — line text is free-form.
- **Usage metering.** No automated quantity inference; quantities are fixed
  at schedule-creation time.

If you need any of the above, materialise drafts manually (or via a sister
service) and use `inv` only for the `draft → issued → sent → paid` lifecycle.
