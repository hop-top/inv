# Agency owner (Algiers, fin+inv local)

> Agency owner in Algiers (wilaya 16 — `DZ-16`). Billing local DZ clients
> (TVA 19% / reduced 9%) and DZ-export clients (zero-rated). Runs `inv`
> via CLI for ad-hoc invoices; recurring monthly retainers via schedules.
> Hands a CSV to the accountant at month-end.

## Context

- Seller jurisdiction `DZ-16`. Buyers split between DZ (any wilaya — TVA is
  national, same row applies) and foreign clients (treated as `export`,
  0% TVA).
- `fin` runs alongside, posts charges, receives payments. Payments often
  come in via bank transfer with a manual reconciliation step in `fin`;
  `fin.billing.payment.received` then advances the `inv` state.
- DZD currency for local clients (0 decimals), USD/EUR for export clients
  (2 decimals). Banker's rounding to the currency scale.
- CLI-first. Occasional `inv server` boot to drain the outbox + run the
  schedule and reminders tickers.
- No SMTP. No agent. Delivery via `link://` URLs sent to clients over
  WhatsApp / email by hand.

## What they want from inv

- DZ TVA at 19% standard, 9% reduced — picked correctly per line. See
  [contracts/tax-resolution.md](../contracts/tax-resolution.md) (DZ examples)
  and [reference/tax-tables.md](../reference/tax-tables.md).
- Foreign-buyer zero-rating: `DZ-16 → non-DZ` resolves as `export` → 0% TVA.
  See [contracts/tax-resolution.md#export](../contracts/tax-resolution.md).
- DZD invoices rounded to 0 decimals at the line + total level. See
  [contracts/tax-resolution.md#step-4--compute-amount](../contracts/tax-resolution.md#step-4--compute-amount).
- Recurring monthly retainers via `inv schedule create --cadence
  monthly@<dom> --auto-issue` + reminder ladders on the materialised
  invoices. See [user/how-to-recurring.md](../user/how-to-recurring.md) and
  [reference/cli.md#inv-reminder](../reference/cli.md#inv-reminder).
- Credit notes against paid invoices when a client disputes (the `void`
  path is closed once payment lands; credit-note is the only correct verb).
  See [contracts/creditnote-state-fsm.md](../contracts/creditnote-state-fsm.md).
- CSV-shaped history for the accountant: `inv invoice list --format json |
  jq …` → CSV via a one-liner. See [reference/cli.md](../reference/cli.md)
  (global `--format` flag).

## What they don't need (or want hidden)

- US economic-nexus configuration. Irrelevant for a DZ seller; leave
  `[nexus.*]` everywhere disabled.
- HTTP / WS / MCP adapters. CLI is the whole surface.
- Webhook delivery. No downstream service to call back.
- US-DE / CA-QC tax rows. They sit in the table unused; no harm but no
  benefit.

## Known gaps for this persona at v1

- **No DZ city / wilaya local tax.** DZ TVA is modelled as national in `inv`,
  which matches reality — but if Algiers ever ships municipal levies, the
  table shape is forward-compatible (`applies_to_buyer.locality` is reserved).
  See [contracts/tax-resolution.md#shortcuts-and-disclaimers-design-63--explicit](../contracts/tax-resolution.md#shortcuts-and-disclaimers-design-63--explicit).
- **No SMTP delivery.** Workaround: `link://` + manual paste via WhatsApp /
  SMS / email. The default `[link].token_ttl = 30d` is the right horizon
  for a "view-your-invoice" link sent over messaging.
- **No automated CSV export verb.** `inv invoice list --format json | jq -r
  '...'` is the path; no native `inv invoice export --csv` at v1.
- **Outbox does not relay in CLI-only mode.** Same constraint as the QC
  freelancer — `inv server` must be alive when `fin` publishes payment
  events.

## Reading path

1. [user/tutorial-getting-started.md](../user/tutorial-getting-started.md) — first invoice end-to-end.
2. [user/how-to-tax-config.md](../user/how-to-tax-config.md) — confirm DZ rows in the loaded table; add a reduced-rate line if needed.
3. [contracts/tax-resolution.md](../contracts/tax-resolution.md) — DZ → DZ standard, DZ → DZ reduced, DZ → non-DZ export.
4. [user/how-to-issue.md](../user/how-to-issue.md) — CLI draft → issue → send via `link://`.
5. [user/how-to-recurring.md](../user/how-to-recurring.md) — monthly retainer with `--auto-issue` + the reminders ticker.
6. [contracts/creditnote-state-fsm.md](../contracts/creditnote-state-fsm.md) — when `void` is closed and credit-note is the only verb.
7. [reference/cli.md](../reference/cli.md) — global `--format json` flag for the CSV-export workflow.
8. [reference/event-bus.md](../reference/event-bus.md) — `fin → inv` payment auto-marking; outbox / server requirement.
