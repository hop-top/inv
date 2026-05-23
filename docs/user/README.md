# User guide

For operators and integrators running `inv` end-to-end. If you want to know
*how to do a thing*, you're in the right tree.

## Start here

1. [Install + first invoice in 15 minutes](tutorial-getting-started.md)
2. [Issue an invoice](how-to-issue.md) — CLI, HTTP API, and MCP
3. [Set up recurring billing](how-to-recurring.md)
4. [Configure tax tables + nexus](how-to-tax-config.md)
5. [Troubleshooting](troubleshooting.md)

## Read by intent

| Intent | Read |
|---|---|
| Run my first command | [tutorial-getting-started.md](tutorial-getting-started.md) |
| Draft → issue → send an invoice | [how-to-issue.md](how-to-issue.md) |
| Create a monthly schedule that auto-issues | [how-to-recurring.md](how-to-recurring.md) |
| Add a US state to the nexus list | [how-to-tax-config.md](how-to-tax-config.md) |
| Bill in DZD or CAD | [how-to-issue.md](how-to-issue.md) (currency section) |
| Resolve a foreign-key error on customer delete | [troubleshooting.md](troubleshooting.md) |
| Understand why an invoice has `nexus_review = true` | [troubleshooting.md](troubleshooting.md) |

## Surface map

`inv` exposes the same operations through five channels:

| Channel | Reach via |
|---|---|
| CLI | `inv invoice ...`, `inv schedule ...`, `inv customer ...`, … see [reference/cli.md](../reference/cli.md) |
| HTTP API | `POST /v1/invoices`, `GET /v1/customers/:id`, … see [reference/api.md](../reference/api.md) |
| WebSocket | `{op: "invoice.draft", payload: {…}}` frames + topic subscriptions, see [reference/ws.md](../reference/ws.md) |
| MCP | `inv_invoice_draft` tool + `inv://<kind>/<id>` resources, see [reference/mcp.md](../reference/mcp.md) |
| Event bus | Consumes `fin.billing.*`, emits `inv.billing.*`, see [reference/event-bus.md](../reference/event-bus.md) |

Pick whichever channel matches your context — they're behaviour-identical, all
backed by the same `hop-top-inv-commands` core.

## Conventions in the user guide

- Code blocks that begin with `$` are shell commands.
- Code blocks with no prompt are config snippets or JSON payloads.
- Output samples are illustrative, not byte-exact — `inv` will print pretty
  tables in a TTY and JSON when piped (`--format json`).
