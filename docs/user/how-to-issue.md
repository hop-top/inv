# How to issue + send an invoice

For an operator (or agent) who knows what an invoice is and wants the
copy-pasteable recipe to draft → issue → send → mark-paid via each channel.

## Decision: which channel?

| You are... | Use |
|---|---|
| At a shell | CLI: `inv invoice ...` |
| Calling from another service | HTTP API: `POST /v1/invoices/...` |
| Building a real-time dashboard | WebSocket: `{op: "invoice.draft", ...}` + topic subscriptions |
| An LLM agent with MCP tools | MCP: `inv_invoice_draft` |

All four call into the same `hop-top-inv-commands` functions. Behaviour is identical;
only the request decode + response encode differ.

## CLI

### Draft

```sh
$ inv invoice draft \
    --customer customer_01... \
    --currency CAD \
    --seller-jurisdiction CA-QC \
    --line "Consulting:10:125.00" \
    --line "Travel:1:200.00" \
    --due 2026-06-30T00:00:00Z \
    --idempotency-key "draft-2026-06-acme-1"
```

The `--line` argument is repeatable. The colon-delimited form is
`<description>:<quantity>:<unit_price>`. Lines start as `tax_category =
standard`; specify per-line categories through the API or WS payload.

`--idempotency-key` is optional but recommended in scripts: a second draft
with the same key returns the original output (the existing invoice id), it
does not create a duplicate.

### Issue

```sh
$ inv invoice issue invoice_01...
```

Issuing assigns a number (e.g. `INV-2026-0001`), freezes tax, renders the
HTML + PDF, and stores the PDF as a blob. State: `draft → issued`.

### Send

```sh
$ inv invoice send invoice_01... --to file:///tmp/inv.pdf
$ inv invoice send invoice_01... --to stdout
$ inv invoice send invoice_01... --to bus://
$ inv invoice send invoice_01... --to webhook://example.com/billing/hook
$ inv invoice send invoice_01... --to link://
```

- `file://path` — writes the PDF to disk.
- `stdout` — writes PDF bytes to stdout (pipe to `pdftotext`, `lp`, etc.).
- `bus://` — emits `inv.billing.invoice.sent` with the PDF inline (base64).
- `webhook://url` — POSTs `{ invoice, pdf_url, signature }` to the URL.
  The webhook receives an `X-Inv-Signature: sha256=<hex>` header (HMAC over
  the body).
- `link://` — returns a signed shareable URL served by `inv server`'s
  public-view route. See [contracts/signed-link-token.md](../contracts/signed-link-token.md).

Re-sending an `issued` or already-`sent` invoice transitions to (or stays in)
`sent` — it never moves backwards. Subsequent re-sends emit `.sent` again
without changing state. After payment, `send` becomes a no-op state-wise (the
PDF still renders).

### Pay

```sh
$ inv invoice pay invoice_01... --amount 1450.00
$ inv invoice pay invoice_01... --amount 500.00      # partial; state → partially_paid
$ inv invoice pay invoice_01... --amount 950.00      # remainder; state → paid
```

A non-positive amount is rejected. Subsequent payments accumulate against
`amount_paid`; the FSM classifies the result as `Paid` (cumulative ≥ total)
or `PartiallyPaid` (cumulative < total). Banker's rounding to the currency
scale: 2 decimals for USD/CAD, 0 decimals for DZD.

### Void

```sh
$ inv invoice void invoice_01... --reason "Customer cancelled before delivery"
```

`void` is allowed from `issued`, `sent`, `viewed`, or `partially_paid` ONLY
when `amount_paid == 0`. After any payment lands, void is rejected — issue a
credit note instead (`inv creditnote draft --invoice ...`).

## HTTP API

The server must be running: `inv server --listen 127.0.0.1:7400`.

Every `/v1/*` route requires `Authorization: Bearer <token>` (token list is
config-driven). Error responses use RFC 9457 problem-detail.

### Draft

```sh
$ curl -X POST http://127.0.0.1:7400/v1/invoices \
    -H "Authorization: Bearer dev-token" \
    -H "Content-Type: application/json" \
    -d '{
      "customer_id": "customer_01...",
      "currency": "CAD",
      "seller_jurisdiction": "CA-QC",
      "lines": [
        {"description": "Consulting", "quantity": "10", "unit_price": "125.00"},
        {"description": "Travel",     "quantity": "1",  "unit_price": "200.00"}
      ],
      "due_at": "2026-06-30T00:00:00Z",
      "idempotency_key": "draft-2026-06-acme-1"
    }'
```

### Issue / send / pay / void

```sh
$ curl -X POST http://127.0.0.1:7400/v1/invoices/invoice_01.../issue \
       -H "Authorization: Bearer dev-token"

$ curl -X POST http://127.0.0.1:7400/v1/invoices/invoice_01.../send \
       -H "Authorization: Bearer dev-token" \
       -H "Content-Type: application/json" \
       -d '{"destination_uri": "file:///tmp/inv.pdf"}'

$ curl -X POST http://127.0.0.1:7400/v1/invoices/invoice_01.../pay \
       -H "Authorization: Bearer dev-token" \
       -H "Content-Type: application/json" \
       -d '{"amount": "1450.00"}'

$ curl -X POST http://127.0.0.1:7400/v1/invoices/invoice_01.../void \
       -H "Authorization: Bearer dev-token" \
       -H "Content-Type: application/json" \
       -d '{"reason": "Customer cancelled"}'
```

The full route table — including credit notes, schedules, reminders, and
customers — is in [reference/api.md](../reference/api.md).

## MCP

If your agent has access to `inv server --mcp` (stdio transport), the
following tools are available:

```
inv_invoice_draft       create a draft
inv_invoice_issue       transition to issued
inv_invoice_send        deliver to URI
inv_invoice_pay         record payment
inv_invoice_void        terminal pre-payment void
inv_invoice_show        single invoice (no lines)
inv_invoice_list        filtered list
inv_creditnote_draft    credit a paid invoice
inv_creditnote_issue
inv_customer_add
inv_customer_show
inv_customer_list
inv_schedule_create
inv_schedule_pause
inv_schedule_cancel
inv_reminder_schedule
inv_reminder_cancel
inv_tick_schedules      manual ticker (mostly for tests)
inv_tick_reminders
inv_tick_overdue
inv_tax_rates_show
```

Each tool accepts a JSON-schema'd input that mirrors the matching CLI flags.
Read-only resource URIs: `inv://invoice/<id>`, `inv://creditnote/<id>`,
`inv://schedule/<id>`, `inv://reminder/<id>`, `inv://customer/<id>`. See
[reference/mcp.md](../reference/mcp.md).

## What you'll see on the bus

For a draft → issue → send → pay journey, the topics in emission order:

```
inv.billing.invoice.proposed       # mechanic, veto-able, pre-draft
inv.billing.invoice.transitioned   # mechanic, post-draft
inv.billing.invoice.entered        # mechanic, post-draft
inv.billing.invoice.drafted        # domain
inv.billing.invoice.proposed       # mechanic, pre-issue
inv.billing.invoice.transitioned
inv.billing.invoice.entered
inv.billing.invoice.issued         # domain
... (sent + paid follow the same triplet+domain pattern)
```

Domain events describe *what happened in business terms*. Mechanic events
describe *how the FSM moved*. Both are useful — subscribe to whatever you
need. See [reference/event-bus.md](../reference/event-bus.md) for payload
shapes.

## See also

- [Recurring billing](how-to-recurring.md) — same `draft / issue` operations
  but driven by a schedule.
- [Tax config](how-to-tax-config.md) — how rates are picked.
- [Invoice FSM](../contracts/invoice-state-fsm.md) — every legal transition.
