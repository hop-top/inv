# CLI reference

Every `inv` subcommand and flag. Authored from
[`crates/cli/src/commands/*.rs`](../../crates/cli/src/commands/).

Global flags (apply to every subcommand):

- `--config <path>` — path to `inv.toml`. Default:
  `$XDG_CONFIG_HOME/inv/config.toml`, falling back to
  `$HOME/.config/inv/config.toml`.
- `--format {table | json | yaml}` — output format (from
  `hop_top_kit::output`). Default: `table` in a TTY, `json` when piped.
  Companion flags `--format-opt`, `--format-help`, `--cols`, `--template`
  are also registered globally — run `inv --format-help` for the full
  flag suite.

## Top-level subcommands

| Subcommand | Purpose |
|---|---|
| `inv invoice ...` | Invoice lifecycle. |
| `inv creditnote ...` | Credit-note lifecycle. |
| `inv schedule ...` | Recurring schedules. |
| `inv reminder ...` | Invoice reminders. |
| `inv customer ...` | Customer CRUD. |
| `inv tax ...` | Inspect the loaded tax table. |
| `inv tick ...` | Manually trigger an internal ticker. |
| `inv server` | Long-lived process (HTTP + WS + tickers + outbox). |

---

## `inv invoice`

### `inv invoice draft`

Create a draft invoice.

| Flag | Required | Description |
|---|---|---|
| `--customer <typeid>` | yes | Customer typeid. |
| `--currency <code>` | yes | ISO 4217 (USD, CAD, DZD). |
| `--seller-jurisdiction <jur>` | no | Default `CA-QC`. One of `CA-QC | US-DE | DZ-16`. |
| `--line "<desc>:<qty>:<price>"` | yes | Repeatable; ≥ 1 required. |
| `--due <rfc3339>` | no | Due date. |
| `--idempotency-key <key>` | no | Caller-supplied dedup key. |
| `--template-path <path>` | no | Per-invoice template override. |

### `inv invoice issue <id>`

Transition `draft → issued`. Assigns a number, freezes tax, renders PDF.

### `inv invoice send <id> --to <uri>`

| Flag | Required | Description |
|---|---|---|
| `<id>` (positional) | yes | Invoice typeid. |
| `--to <uri>` | yes | One of `file://<path>`, `stdout`, `bus://`, `webhook://<url>`, `link://`. |

### `inv invoice pay <id>`

| Flag | Required | Description |
|---|---|---|
| `<id>` (positional) | yes | Invoice typeid. |
| `--amount <decimal>` | yes | Positive payment amount. |
| `--idempotency-key <key>` | no | Caller-supplied dedup key. |
| `--bus-event-id <id>` | no | Inbound bus event id (rarely supplied from CLI). |

### `inv invoice void <id>`

| Flag | Required | Description |
|---|---|---|
| `<id>` (positional) | yes | Invoice typeid. |
| `--reason <text>` | no | Free-form void reason. |

### `inv invoice show <id>`

Print one invoice with lines.

### `inv invoice list`

| Flag | Description |
|---|---|
| `--customer <typeid>` | Filter by customer. |
| `--state <state>` | One of `draft | issued | sent | viewed | partially_paid | paid | voided`. |
| `--limit <n>` | Pagination limit. |
| `--offset <n>` | Pagination offset. |

---

## `inv creditnote`

### `inv creditnote draft`

| Flag | Required | Description |
|---|---|---|
| `--invoice <typeid>` | yes | Invoice to credit. |
| `--amount <decimal>` | yes | Credit amount (positive). |
| `--reason <text>` | no | Free-form reason. |
| `--idempotency-key <key>` | no | Caller-supplied dedup key. |

### `inv creditnote issue <id>`

`draft → issued` (terminal). Assigns `CN-YYYY-NNNN`.

### `inv creditnote show <id>`

### `inv creditnote list --invoice <typeid>`

List credit notes for an invoice.

---

## `inv schedule`

### `inv schedule create`

| Flag | Required | Description |
|---|---|---|
| `--customer <typeid>` | yes | Customer typeid. |
| `--currency <code>` | yes | ISO 4217. |
| `--cadence <spec>` | yes | `monthly@<dom>` / `quarterly@<dom>` / `yearly@MM-DD` (dom 1–28). |
| `--start <YYYY-MM-DD>` | yes | First cycle date. |
| `--end <YYYY-MM-DD>` | no | Last cycle date. |
| `--auto-issue` | no | Boolean flag: auto-transition `draft → issued` each cycle. |
| `--line "<desc>:<qty>:<price>"` | yes | Repeatable template line; ≥ 1 required. |

### `inv schedule pause <id>`

Active → paused (ticker skips it).

### `inv schedule cancel <id>`

Terminal.

### `inv schedule show <id>`

### `inv schedule list --customer <typeid>`

**At v1, `--customer` is required.** Cross-customer scan is not implemented.

---

## `inv reminder`

### `inv reminder schedule`

| Flag | Required | Description |
|---|---|---|
| `--invoice <typeid>` | yes | Invoice typeid. |
| `--at <rfc3339>` | yes | Dispatch time (must be in the future). |
| `--channel <name>` | yes | One of `file | stdout | bus | webhook | link`. |

### `inv reminder cancel <id>`

### `inv reminder list --invoice <typeid>`

---

## `inv customer`

### `inv customer add`

| Flag | Required | Description |
|---|---|---|
| `--name <text>` | yes | Display name on rendered invoices. |
| `--email <addr>` | no | Optional email. |
| `--country <iso2>` | yes | ISO 3166-1 alpha-2. |
| `--region <code>` | no | ISO 3166-2 subdivision (e.g. `QC`). |
| `--city <text>` | no | City. |
| `--postal <text>` | no | Postal code. |
| `--line1 <text>` | no | Address line 1. |
| `--line2 <text>` | no | Address line 2. |

### `inv customer show <id>`

### `inv customer list`

| Flag | Description |
|---|---|
| `--limit <n>` | Default 100. |
| `--offset <n>` | Default 0. |

---

## `inv tax`

### `inv tax rates show`

Dump the loaded tax table (rates only at v1; the `[nexus.*]` config is in the
config file, not surfaced through this command).

---

## `inv tick`

Manually run an internal ticker once. Used in tests and operator chores;
production runs invoke these on a timer from `inv server`.

| Subcommand | Description |
|---|---|
| `inv tick schedules` | Materialise invoices for every active schedule with `next_run <= today`. |
| `inv tick reminders` | Dispatch reminders with `scheduled_at <= now` and `state = scheduled`. |
| `inv tick overdue` | Flag `Issued | Sent | Viewed | PartiallyPaid` invoices past their due date. |

---

## `inv server`

Run the long-lived process.

| Flag | Default | Description |
|---|---|---|
| `--listen <addr>` | `127.0.0.1:7400` | HTTP + WS bind address. |
| `--mcp` | off | Serve MCP over stdio (silences stdout tracing). |
| `--outbox-interval-secs <n>` | `5` | Bus outbox drain interval. |
| `--schedules-interval-secs <n>` | `86400` | Schedule materialisation tick (daily). |
| `--reminders-interval-secs <n>` | `300` | Reminder dispatch tick (5 min). |
| `--overdue-interval-secs <n>` | `3600` | Overdue flag tick (hourly). |

Topology and graceful-shutdown semantics are documented inline in
[`crates/cli/src/commands/server.rs`](../../crates/cli/src/commands/server.rs).

---

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success. |
| Non-zero | Underlying error printed to stderr (anyhow chain, `--config` parse error, command error). |

Error messages from the command layer surface `CoreError` variants — the
mapping to HTTP / WS / MCP error codes is in the matching reference page.
