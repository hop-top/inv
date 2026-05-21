# Tutorial: your first invoice in 15 minutes

For an operator who wants to install `inv`, create a customer, draft + issue +
send an invoice, and watch the bus events emerge. By the end you'll have a
working sqlite store, a rendered PDF in `/tmp`, and a feel for the surface.

## What you'll have at the end

- A working `inv` binary on `$PATH`.
- A local sqlite DB at `./inv.sqlite` with one customer and one paid invoice.
- A PDF on disk at `/tmp/first-invoice.pdf`.
- Six `inv.billing.*` events emitted (drafted → issued → sent → paid; plus
  mechanic `.transitioned` / `.entered`).

## Prerequisites

- Rust 1.85+ with `cargo`.
- `git` + a writable working directory.
- (Optional) `wkhtmltopdf` or `weasyprint` on `$PATH` if you want a real PDF
  engine — otherwise `inv` falls back to its bundled pure-Rust renderer.

## Step 1 — install

```sh
$ git clone https://github.com/hop-top/inv.git
$ cd inv
$ cargo build --release
$ install -m 0755 target/release/inv /usr/local/bin/inv     # or your $PATH dir
```

Verify:

```sh
$ inv --version
inv 0.0.0
$ inv --help
```

You should see the eight top-level subcommands: `invoice`, `creditnote`,
`schedule`, `reminder`, `customer`, `tax`, `tick`, `server`.

## Step 2 — minimal config (optional)

`inv` runs out of the box against an in-memory sqlite DB. For a persistent
store, write `~/.config/inv/config.toml`:

```toml
[storage]
backend = "sqlite"
dsn = "sqlite:./inv.sqlite"

[tax]
tax_tables = "tax-tables/default.toml"
```

Full schema: [reference/config.md](../reference/config.md).

## Step 3 — add a customer

```sh
$ inv customer add \
    --name "Acme Co." \
    --email billing@acme.example \
    --country CA \
    --region QC \
    --city Montreal \
    --postal "H2Y 1C6"
```

Output (table by default; add `--format json` for machine-readable):

```text
id                          display_name          email
customer_01j9zk...          Acme Co.              billing@acme.example
```

Copy the `id` — you'll need it.

## Step 4 — draft an invoice

```sh
$ inv invoice draft \
    --customer customer_01j9zk... \
    --currency CAD \
    --seller-jurisdiction CA-QC \
    --line "Consulting:10:125.00" \
    --line "Travel:1:200.00" \
    --due 2026-06-30T00:00:00Z
```

The `--line` syntax is `<description>:<quantity>:<unit-price>` (repeatable).
The output JSON includes the new invoice id + the emitted topics:

```json
{
  "invoice": { "id": "invoice_01j9zl...", "state": "draft", "total": "1450.00" },
  "emitted_events": ["inv.billing.invoice.drafted", "inv.billing.invoice.proposed", "inv.billing.invoice.transitioned", "inv.billing.invoice.entered"]
}
```

## Step 5 — issue + send

```sh
$ inv invoice issue invoice_01j9zl...
$ inv invoice send invoice_01j9zl... --to file:///tmp/first-invoice.pdf
```

After `issue`, the invoice gets a number (`INV-2026-0001`), tax is frozen, and
the PDF is rendered. After `send`, the PDF is written to the URI — `file://`
to disk, `stdout` to standard output, `webhook://` POSTed as JSON,
`link://` returns an HMAC-signed URL served by `inv server`'s public-view
route.

Emitted topics on send:

- `inv.billing.invoice.sent` (domain)
- `inv.billing.invoice.proposed` + `.transitioned` + `.entered` (mechanic)

## Step 6 — record a payment

```sh
$ inv invoice pay invoice_01j9zl... --amount 1450.00
```

Final state: `paid`. Topic: `inv.billing.invoice.paid`.

You can verify the full audit trail by querying `invoice_state_history` in the
sqlite store:

```sh
$ sqlite3 ./inv.sqlite "SELECT from_state, to_state, event, channel, occurred_at FROM invoice_state_history WHERE invoice_id = 'invoice_01j9zl...' ORDER BY occurred_at"
```

## Step 7 — boot the server (optional)

To run `inv` as a long-lived process with the HTTP API + WebSocket on a
single listener, the tickers, and the outbox relay:

```sh
$ inv server --listen 127.0.0.1:7400
HTTP + WS listening on 127.0.0.1:7400
```

Now the same operations are reachable via HTTP:

```sh
$ curl -X POST http://127.0.0.1:7400/v1/invoices/invoice_01j9zl.../issue \
       -H "Authorization: Bearer dev-token"
```

The full route list is in [reference/api.md](../reference/api.md).

## Next

- Set up [recurring billing](how-to-recurring.md) on a monthly cadence.
- Configure [tax rates](how-to-tax-config.md) for your jurisdiction.
- Learn the [issue / send variants](how-to-issue.md) (link, webhook).
- Read the [invoice FSM contract](../contracts/invoice-state-fsm.md) to know
  exactly which transitions are legal and which aren't.

## If something broke

Go to [troubleshooting](troubleshooting.md). The top failure modes
(missing customer, nexus undetermined, idempotency replay, FK violation on
delete) are all documented with copy-pasteable fixes.
