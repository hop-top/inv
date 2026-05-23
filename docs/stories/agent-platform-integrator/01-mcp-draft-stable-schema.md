# Story agent-platform-integrator-01: MCP tool inv_invoice_draft exposes a stable JsonSchema

**Persona**: [Agent platform integrator](../../personas/agent-platform-integrator.md)

## Story

As an engineer embedding `inv` as an MCP tool in an agent app, I want the
`inv_invoice_draft` tool's input schema to be discoverable via
`schemars::JsonSchema`, stable across patch releases, and identical in shape
to the matching HTTP and CLI request, so that an agent picking the tool
doesn't have to reason about per-channel quirks.

## Acceptance criteria

**Given** the agent boots an MCP client against `inv server --mcp` (stdio
transport)
**When** the agent calls `tools/list`
**Then** the response includes `inv_invoice_draft` with an `inputSchema`
that round-trips through `serde_json` and matches the field set documented
in [reference/mcp.md#invoice](../../reference/mcp.md#invoice): `customer_id`,
`currency`, `seller_jurisdiction?`, `lines[]`, `due_at?`,
`idempotency_key?`, `template_path?`. Field names use snake_case, money
fields are quoted decimal strings, and the schema is derivable from the
matching `DraftInvoiceInput` type in `hop-top-inv-commands`.

**Given** the agent calls
```json
{
  "method": "tools/call",
  "params": {
    "name": "inv_invoice_draft",
    "arguments": {
      "customer_id": "customer_01...",
      "currency": "CAD",
      "seller_jurisdiction": "CA-QC",
      "lines": [
        {"description": "Consulting", "quantity": "10", "unit_price": "125.00"}
      ]
    }
  }
}
```
**Then** the tool returns a result payload whose shape mirrors the HTTP
`POST /v1/invoices` response (per [reference/api.md](../../reference/api.md))
and includes the created invoice's typeid + the
`EmittedEvent` list (see [story 03](03-emitted-events-in-tool-output.md)).

**Given** the agent calls the tool with malformed input (e.g. `currency` is
a number, or `lines` is empty)
**When** the tool processes the call
**Then** the response is an `McpError::Decode` carrying the JSON-schema
validation message (per [reference/mcp.md#error-envelope](../../reference/mcp.md#error-envelope)).
The error envelope is identical to the corresponding HTTP `400 validation`
problem-detail in semantic, so the agent can map errors generically.

## Surfaces touched

- MCP — `tools/list`, `tools/call` for `inv_invoice_draft`.
- Commands — `draft_invoice` (the same function HTTP and CLI call).
- Schema — `schemars::JsonSchema` derive on `DraftInvoiceInput`.

## Out of scope for this story

- HTTP/SSE MCP transport — stdio only at v1.
- Schema versioning / migration — no SemVer story for tool input shape
  changes at v1-alpha. Treat any change as breaking; track via the
  workspace `CARGO_PKG_VERSION` advertised in `get_info()`.
- Tool-side rate limiting — no backstop.

## See also

- [reference/mcp.md](../../reference/mcp.md) — every tool + resource template + capabilities.
- [architecture/design-spec.md §3](../../architecture/design-spec.md) — one core, thin adapters.
- [contracts/invoice-state-fsm.md](../../contracts/invoice-state-fsm.md) — same illegal transitions surface identically across MCP / HTTP / CLI.
