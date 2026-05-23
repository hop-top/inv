# MCP reference

Every tool + resource template `hop-top-inv-mcp` exposes. Authored from
[`crates/mcp/src/server.rs`](../../crates/mcp/src/server.rs),
[`crates/mcp/src/tools/`](../../crates/mcp/src/tools/), and
[`crates/mcp/src/resources.rs`](../../crates/mcp/src/resources.rs).

## Transport

`stdio` only at v1. Boot via `inv server --mcp` (the flag silences stdout
tracing, since the JSON-RPC stream shares stdout). HTTP / SSE transports
land in v1.1.

```sh
$ inv server --mcp
# JSON-RPC over stdio. Pipe to your MCP client of choice.
```

Programmatic boot:

```rust
use std::sync::Arc;
use hop_top_inv_commands::CoreCtx;

# async fn run(ctx: Arc<CoreCtx>) -> anyhow::Result<()> {
hop_top_inv_mcp::run_stdio(ctx).await?;
# Ok(()) }
```

## Tools (20 total)

Every command in `hop-top-inv-commands` is exposed as one MCP tool. Tool inputs use
`schemars::JsonSchema` to publish a typed JSON schema; payloads mirror the
matching CLI / WS request shapes.

### Invoice

| Tool | Description |
|---|---|
| `inv_invoice_draft` | Create a draft. Lines pre-tax; tax frozen at issue. |
| `inv_invoice_issue` | `draft → issued`. Assigns number, snapshots tax, renders HTML+PDF. |
| `inv_invoice_send` | Deliver to URI. v1 supports `file://`, `stdout`. (`bus://` / `webhook://` / `link://` are routed but the stdout sink captures in-memory under the MCP transport.) |
| `inv_invoice_pay` | Record payment. > 0; classifies as `PartiallyPaid` or `Paid`. |
| `inv_invoice_void` | Pre-payment void only. Terminal. |
| `inv_invoice_show` | Single invoice (no lines; use the `inv://invoice/<id>` resource for full detail). |
| `inv_invoice_list` | List with optional `customer_id`, `state`, `limit`, `offset` filters. |

### Credit note

| Tool | Description |
|---|---|
| `inv_creditnote_draft` | Against an invoice. Amount > 0. |
| `inv_creditnote_issue` | `draft → issued`. Assigns `CN-YYYY-NNNN`. Terminal. |

### Customer

| Tool | Description |
|---|---|
| `inv_customer_add` | Upsert by id; mints fresh id when none supplied. |
| `inv_customer_show` | Single customer. |
| `inv_customer_list` | Paged; default limit 50. |

### Schedule

| Tool | Description |
|---|---|
| `inv_schedule_create` | Cadence: `monthly@<dom>` / `quarterly@<dom>` / `yearly@MM-DD`. |
| `inv_schedule_pause` | Active → paused (ticker skips). Idempotent. |
| `inv_schedule_cancel` | Terminal. Idempotent on already-cancelled. |

### Reminder

| Tool | Description |
|---|---|
| `inv_reminder_schedule` | Against an invoice. `scheduled_at` must be future. |
| `inv_reminder_cancel` | No-op if already sent or cancelled. |

### Tickers (parameter-free)

| Tool | Description |
|---|---|
| `inv_tick_schedules` | Materialise invoices for schedules with `next_run <= today`. |
| `inv_tick_reminders` | Dispatch reminders with `scheduled_at <= now`. |
| `inv_tick_overdue` | Emit `.overdue` for `Issued/Sent/Viewed/PartiallyPaid` invoices past their due date. |

### Tax

| Tool | Description |
|---|---|
| `inv_tax_rates_show` | Loaded tax-rate table + US nexus config. |

## Resource templates (5)

Read-only views over the persisted entities. Each template URI has `<id>`
replaced by the entity's typeid before being read.

| Template | Returns |
|---|---|
| `inv://invoice/<id>` | Full invoice JSON. |
| `inv://creditnote/<id>` | Credit-note JSON. |
| `inv://schedule/<id>` | Schedule JSON. |
| `inv://reminder/<id>` | Reminder JSON. |
| `inv://customer/<id>` | Customer JSON. |

MIME type: `application/json`.

The resources/list endpoint returns an empty list at v1 — clients should use
the **templates** + the read endpoint with substituted IDs. The template list
is the canonical surface.

## Capability advertisement

`get_info()` returns `ServerInfo` with:

- `enable_tools()`
- `enable_tool_list_changed()`
- `enable_resources()`

Server identifier: `hop-top-inv-mcp` at the workspace `CARGO_PKG_VERSION`.

## Error envelope

`McpError` (from [`crates/mcp/src/error.rs`](../../crates/mcp/src/error.rs))
maps to rmcp's wire-level error:

| McpError | Cause |
|---|---|
| `InvalidUri` | Resource URI doesn't parse as `inv://<kind>/<id>`. |
| `NotFound` | Resource not in store. |
| `Decode` | Tool input failed JSON schema validation. |
| `Repo` | DB / repo failure. |
| `Command` | `hop-top-inv-commands` returned `CoreError`. |

## See also

- [WS reference](ws.md) — same ops over a different transport.
- [Bus payload schemas](../contracts/bus-payload-schemas.md) — what tools
  emit in their `emitted_events` field.
