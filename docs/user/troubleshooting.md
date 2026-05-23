# Troubleshooting

For operators when something goes wrong. Each entry: symptom → cause → fix.

If your symptom isn't here, check:

- [reference/cli.md](../reference/cli.md) for flag spelling.
- [contracts/invoice-state-fsm.md](../contracts/invoice-state-fsm.md) for "is
  this transition legal?".
- [reference/event-bus.md](../reference/event-bus.md) for "what topic does X
  emit?".
- The source path each doc page cites at the top.

---

## "FOREIGN KEY constraint failed" on customer delete

**Symptom.** Deleting a customer fails with a sqlite FK error.

**Cause.** The customer is referenced by `invoices.customer_id` or
`schedules.customer_id`. Those refs are `REFERENCES customers(id)` (no
`ON DELETE CASCADE` — design §5).

**Fix.**

```sh
$ inv invoice list --customer customer_01...
$ inv schedule list --customer customer_01...
```

Cancel any active schedules (`inv schedule cancel <id>`), void any unpaid
invoices (`inv invoice void <id>`), and either credit-note + archive the paid
ones, or leave them — paid invoices are an audit asset; the rule of thumb is
"customers leave the system soft-deleted, never hard-deleted". Hard-delete
the customer row directly via SQL if you truly need to, accepting the audit
loss.

---

## An invoice shows `nexus_review = true`

**Symptom.** A US-DE → US (non-DE) invoice was issued with `tax_total = 0`
and `nexus_review = true`.

**Cause.** The buyer's state has `[nexus.<STATE>] enabled = false` in your
tax table, so `inv` didn't apply the state's sales-tax row. The flag is the
*opposite* of an error — it's a signal for operator review.

**Fix.** If you have economic nexus in the destination state, opt in:

```toml
[nexus.CA]
enabled = true
revenue = "500000.00"
```

Then re-issue affected invoices (you'll need to void the existing draft, edit
the table, and re-draft + re-issue — `inv` does not auto-recompute frozen
tax on already-issued invoices). See [how-to-tax-config.md](how-to-tax-config.md).

---

## Idempotency replay returns "the same" output unexpectedly

**Symptom.** You called `draft_invoice` with `idempotency_key = "foo"` twice
expecting two invoices; you got one (the same id twice).

**Cause.** This is by design (spec §3.5 + §4.4). Repeating a command with the
same `idempotency_key` returns the **original** output — same invoice id, no
duplicate row.

**Fix.** Use a fresh key per intent. Don't reuse keys across distinct
invoices. The output payload includes `idempotency_replay: true` when you're
getting a replay rather than a fresh result — check that flag in scripts.

---

## "illegal transition: ... not allowed from Paid"

**Symptom.** `inv invoice void invoice_01...` errors after the invoice was
already paid.

**Cause.** `void` is allowed only **before** any payment is recorded. From
`Paid` (or `PartiallyPaid` with `amount_paid > 0`), it's rejected by the FSM.

**Fix.** Issue a **credit note** instead:

```sh
$ inv creditnote draft --invoice invoice_01... --amount 1450.00 --reason "Customer cancelled"
$ inv creditnote issue creditnote_01...
```

The credit note flips to `issued` and emits `inv.billing.creditnote.issued`.
The original invoice stays `Paid` — credit notes net against it for reporting.

---

## "illegal transition: pay not allowed from Draft"

**Symptom.** Trying to record a payment on a draft.

**Cause.** Drafts aren't yet customer-facing — you can't be paid for something
you haven't issued.

**Fix.** Issue first.

```sh
$ inv invoice issue invoice_01...
$ inv invoice pay   invoice_01... --amount 1450.00
```

---

## "invoice_state_history rows have published_at = NULL forever"

**Symptom.** Events emitted but bus subscribers never see them.

**Cause.** The **outbox relay** isn't running. The relay drains
`invoice_state_history` rows where `published_at IS NULL`, publishes them to
the bus, and marks them processed. It runs only inside `inv server`.

**Fix.** Either:

- Start `inv server` (the relay runs on a 5s default interval; tunable via
  `--outbox-interval-secs`).
- Or, in tests, drain manually:
  `cargo run -p hop-top-inv-bus --example drain_outbox` (helper TBD).

In CLI-only / one-shot mode, the outbox is **persisted** but **not
relayed** — by design. The history table is the durable audit log; you can
backfill subscribers later.

---

## "wkhtmltopdf not found"

**Symptom.** `inv invoice issue` fails with a PDF-engine error.

**Cause.** The PDF engine isn't on `$PATH` and you compiled with
`--features pdf-wkhtmltopdf` or `pdf-weasyprint`.

**Fix.** Install the binary (`brew install wkhtmltopdf` or
`pip install weasyprint`), or rebuild with the pure-Rust default
(`--features pdf-typst` or just `cargo build` without extra features).

PDF engine choice + Cargo features: [reference/config.md](../reference/config.md)
and the design spec §12.

---

## "schedule.create: invalid cadence"

**Symptom.** `inv schedule create --cadence "monthly@31"` errors.

**Cause.** Day-of-month must be 1–28 (months have variable lengths; using
30/31 makes February ambiguous).

**Fix.** Use a DOM ≤ 28, or pick a different cadence. Quarterly and yearly
have the same constraint via their DOM segment.

---

## "Invoice rendered but PDF is corrupt / empty"

**Symptom.** `invoice send --to file://...` writes 0 bytes or a malformed PDF.

**Cause.** The render pipeline failed silently. Check `inv` stderr — the
specific PDF engine emits its own diagnostics. The bundled pure-Rust fallback
is conservative; complex templates (custom fonts, embedded images larger than
~1MB) may need `pdf-wkhtmltopdf` or `pdf-weasyprint`.

**Fix.** Re-run with `RUST_LOG=debug` to see the renderer trace. Swap PDF
engine via Cargo feature if the template is the issue.

---

## "WebSocket op rejected with code: unknown_op"

**Symptom.** WS frame is rejected.

**Cause.** Typo or unsupported op. Op names are exact and dot-segmented:
`invoice.draft`, `invoice.issue`, `invoice.send`, `invoice.pay`,
`invoice.void`, `creditnote.draft`, `creditnote.issue`,
`schedule.create | pause | cancel`, `reminder.schedule | cancel`,
`tick.schedules | reminders | overdue`, `subscribe`, `unsubscribe`,
`ping`. Full list: [reference/ws.md](../reference/ws.md).

**Fix.** Send a `{"id": "1", "op": "ping"}` first to confirm transport health,
then re-check the op name spelling.

## See also

- [Recovery quick map](tutorial-getting-started.md#step-7--boot-the-server-optional)
  in the tutorial.
- [contracts/invoice-state-fsm.md](../contracts/invoice-state-fsm.md) — full
  list of legal vs illegal transitions; if it's illegal, the FSM will reject
  it identically across all five channels.
