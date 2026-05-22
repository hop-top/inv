# Freelancer (Quebec, fin+inv local)

> Solo consultant in QC billing CA + US clients. Runs `fin` and `inv` on a
> laptop; no email infrastructure; payment events flow back through `fin`.

## Context

- Seller jurisdiction `CA-QC`. Clients in QC (GST + QST stacked), other CA
  provinces (destination HST/PST + federal GST), and US (export, 0% sales
  tax — no US nexus).
- `fin` runs on the same machine, owns the ledger, posts charges, receives
  payments (manual import from Stripe / bank statements). `inv` consumes
  `fin.billing.payment.received` to auto-mark invoices paid.
- No `aps`. No agent platform. Reaches `inv` from a shell.
- No SMTP. No mail server. Delivers invoices as signed-link URLs pasted into
  client portals / Slack / SMS by hand.
- Backend stays sqlite. No `inv server` needed for steady state; just CLI +
  the occasional `inv server` boot to drain the outbox + run the schedule
  ticker.

## What they want from inv

- Draft → issue → send via `link://`. The signed URL goes to the client
  out-of-band. See [user/how-to-issue.md](../user/how-to-issue.md) and
  [contracts/signed-link-token.md](../contracts/signed-link-token.md).
- Destination-aware tax: QC → QC stacks GST+QST; QC → ON resolves HST; QC
  → US treats as `export` and applies 0%. See
  [contracts/tax-resolution.md](../contracts/tax-resolution.md).
- Multi-currency: invoice US clients in USD, CA clients in CAD. No FX
  conversion. See design [§2.1](../architecture/design-spec.md#21-in-scope-v1).
- Recurring monthly retainers via `inv schedule create … --auto-issue`. See
  [user/how-to-recurring.md](../user/how-to-recurring.md).
- Reminders on overdue invoices via `link://` channel. See
  [reference/cli.md](../reference/cli.md#inv-reminder).
- Auto-mark-paid when `fin` reports `fin.billing.payment.received`. See
  [reference/event-bus.md](../reference/event-bus.md#consumed-topics).

## What they don't need (or want hidden)

- HTTP / WebSocket adapter. They never call `inv server` for external
  traffic; the only reason to start it is the outbox relay + schedule ticker.
- MCP. No agent in the loop.
- US economic-nexus configuration. No nexus → leave every `[nexus.*]` block
  disabled; `nexus_review = true` on US invoices is the expected signal.
- Plan catalogs, proration, usage metering — out of scope at v1 by design,
  and a freelancer doesn't need them.

## Known gaps for this persona at v1

- **No SMTP/SMS delivery.** Workaround: `link://` + manual paste. See
  [user/troubleshooting.md](../user/troubleshooting.md) for the recurring
  patterns.
- **No automatic running-revenue tracking for nexus.** Not relevant for a QC
  freelancer with no US presence, but worth knowing if the practice scales
  into US-DE territory later.
- **Outbox does not relay in CLI-only mode.** If you never boot `inv
  server`, bus events accumulate in `invoice_state_history` with
  `published_at = NULL`. `fin → inv` payment auto-marking requires the
  consumer running, which means `inv server` must be alive when `fin`
  publishes. Plan a daily cron `inv server` window or run it in the
  background.

## Reading path

1. [user/tutorial-getting-started.md](../user/tutorial-getting-started.md) — first invoice end-to-end.
2. [user/how-to-issue.md](../user/how-to-issue.md) — CLI draft → issue → send with `link://`.
3. [contracts/tax-resolution.md](../contracts/tax-resolution.md) — QC → QC + QC → ON + QC → US worked examples.
4. [user/how-to-recurring.md](../user/how-to-recurring.md) — monthly retainer schedule with `--auto-issue`.
5. [contracts/signed-link-token.md](../contracts/signed-link-token.md) — what the `link://` URL looks like + TTL choice.
6. [reference/event-bus.md](../reference/event-bus.md) — `fin.billing.payment.received` → auto-marked paid.
7. [user/troubleshooting.md](../user/troubleshooting.md) — outbox relay, nexus_review, FK delete errors.
8. [reference/cli.md](../reference/cli.md) — flag-level reference once the patterns above are familiar.
