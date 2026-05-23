# Story agent-platform-integrator-02: Agent reads invoice state via the `inv://invoice/<id>` resource

**Persona**: [Agent platform integrator](../../personas/agent-platform-integrator.md)

## Story

As an engineer building an agent UX, I want the agent to fetch the full
state of an invoice as an MCP resource — without invoking a mutating tool
or maintaining its own cache — so that the LLM can quote the live state
back to the user.

## Acceptance criteria

**Given** an invoice exists at typeid `invoice_01...` in `state = "sent"`
**When** the agent calls
```json
{
  "method": "resources/read",
  "params": { "uri": "inv://invoice/invoice_01..." }
}
```
**Then** the response carries `contents: [{ mimeType: "application/json",
text: "<full invoice JSON>" }]` with state, lines, totals, tax breakdown,
schedule_id (if any), and timestamps — i.e. the same shape as `GET
/v1/invoices/{id}` plus lines, per
[reference/mcp.md#resource-templates-5](../../reference/mcp.md#resource-templates-5).

**Given** the agent calls `resources/read` with a malformed URI like
`inv://wrong/invoice_01...`
**When** the call lands
**Then** the response is `McpError::InvalidUri` carrying the parse failure.

**Given** the agent calls with a well-formed URI for a non-existent invoice
**When** the call lands
**Then** the response is `McpError::NotFound`.

**Given** the agent calls `resources/list`
**When** the call lands
**Then** the response is **empty** (v1 behaviour) — the agent must use the
resource **templates** (`inv://invoice/<id>`) with substituted IDs.
Enumeration of "every invoice" requires a paged `inv_invoice_list` tool
call followed by per-id resource reads.

## Surfaces touched

- MCP — `resources/read`, `resources/list`, the `inv://<kind>/<id>` template
  set (invoice / creditnote / schedule / reminder / customer).
- Commands — read-only repo access via `hop-top-inv-commands`.
- Store — `invoices`, `invoice_lines` (read).

## Out of scope for this story

- Subscriptions / push notifications on resource change — MCP `resources/`
  has no subscribe at v1 (the bus is the right surface for change
  notifications; see [story 03](03-emitted-events-in-tool-output.md)).
- Resource representations beyond JSON — `mimeType` is fixed at
  `application/json`.
- Resources for `invoice_state_history` rows — not exposed at v1; query
  the store directly if you need the audit chain.

## See also

- [reference/mcp.md#resource-templates-5](../../reference/mcp.md#resource-templates-5) — every template + MIME type.
- [reference/api.md](../../reference/api.md) — equivalent `GET /v1/invoices/{id}` route.
- [agent-platform-integrator-01](01-mcp-draft-stable-schema.md) — the tool-side input contract.
