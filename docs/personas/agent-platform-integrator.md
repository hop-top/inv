# Agent platform integrator

> Engineer embedding `inv` as a tool surface inside an agent app. Spawns
> `inv server --mcp` over stdio and binds the resulting tools into an MCP
> client. The agent drives the billing flow on behalf of an end-user.

## Context

- The agent app owns the conversation. `inv` is one of many MCP-exposed
  tools. The agent picks tools by JSON-schema introspection — drift in the
  schema breaks the agent silently.
- `fin` typically also exposed as a tool (if the platform integrates it),
  or alternatively `fin` consumes/emits on the bus and the agent observes
  side-effects via a separate event stream.
- Auth posture varies. Some platforms use stdio (capabilities granted via
  process boundary). Some need a long-lived HTTP transport — at v1 `hop-top-inv-mcp`
  is **stdio only**; HTTP / SSE land in v1.1.
- The integrator is not the end-user. They care about contract stability,
  predictable error surfaces, and replay semantics — not about which
  invoices were issued.

## What they want from inv

- Stable `JsonSchema` for every tool input. `schemars::JsonSchema` is the
  contract surface; changes to a tool's input shape are breaking. See
  [reference/mcp.md](../reference/mcp.md).
- Constant-time bearer comparison (for the day the HTTP transport ships).
  At v1 the same comparison primitive is used by `crates/api`'s bearer
  middleware and the signed-link verifier — see
  [contracts/signed-link-token.md#constant-time-comparison](../contracts/signed-link-token.md#constant-time-comparison)
  and [reference/api.md#auth](../reference/api.md#auth).
- Read-only resources at `inv://<kind>/<id>` for invoice / creditnote /
  schedule / reminder / customer. Agents query state without invoking a
  tool. See [reference/mcp.md#resource-templates-5](../reference/mcp.md#resource-templates-5).
- `EmittedEvent` stream serialisable in tool outputs — every command
  returns the list of bus events it would emit, so the agent can show the
  user what just happened without subscribing to the bus separately. See
  [architecture/design-spec.md §3.5](../architecture/design-spec.md#35-cross-cutting-concerns).
- Replay-safe consumer: `bus_inbox` dedup means re-delivering an event is a
  no-op. Critical for at-least-once bus semantics. See
  [reference/event-bus.md#idempotency--ordering-design-44](../reference/event-bus.md#idempotency--ordering-design-44).
- Predictable error surface: every command failure maps to a typed `McpError`
  variant with an underlying `CoreError`. See
  [reference/mcp.md#error-envelope](../reference/mcp.md#error-envelope).

## What they don't need (or want hidden)

- Channel-specific surface details (CLI flags, HTTP route shape, WS frame
  format). The agent integrator only touches MCP.
- Tax tables and jurisdiction nuance. The end-user configures those via the
  config file out-of-band; the agent doesn't reason about rates.
- PDF rendering pipeline. The agent surfaces `inv_invoice_send` with
  `destination_uri = "stdout"` or `bus://` and lets downstream code handle
  bytes.

## Known gaps for this persona at v1

- **MCP transport is stdio only at v1.** Multi-tenant agent platforms that
  need HTTP/SSE must wait for v1.1. Workaround: shell out per session.
- **`hop-top-inv-mcp` send tool routes `link://` and `webhook://` through an
  in-memory stdout sink.** Confirm in [reference/mcp.md](../reference/mcp.md#tools-20-total)
  before designing flows that depend on real link minting via MCP — at v1,
  the HTTP API is the right surface for `link://`.
- **No tool-side rate limiting.** Agent can hammer `inv_invoice_draft`; the
  only backstop is `idempotency_key` discipline on the agent side.
- **`resources/list` returns empty at v1.** Clients must use the resource
  *templates* (`inv://invoice/<id>`) with substituted IDs — there is no
  enumeration of "every invoice". Build a paged list via
  `inv_invoice_list` and follow the IDs.

## Reading path

1. [reference/mcp.md](../reference/mcp.md) — every tool + every resource template + capabilities + error envelope.
2. [architecture/design-spec.md](../architecture/design-spec.md) §3 + §3.5 — one core, thin adapters; what guarantees the MCP surface inherits.
3. [contracts/invoice-state-fsm.md](../contracts/invoice-state-fsm.md) — every illegal `(state, event)` pair surfaces identically across MCP / HTTP / CLI.
4. [contracts/bus-payload-schemas.md](../contracts/bus-payload-schemas.md) — what `EmittedEvent` lists look like + every consumed payload.
5. [reference/event-bus.md](../reference/event-bus.md) — outbox + inbox semantics; at-least-once subscriber contract.
6. [contracts/signed-link-token.md](../contracts/signed-link-token.md) — constant-time comparison + HMAC token shape (mirror this in your own URL minting if needed).
7. [reference/api.md](../reference/api.md) — fallback surface when MCP transport is the wrong fit; same operations, REST shape.
8. [user/troubleshooting.md](../user/troubleshooting.md) — idempotency replay, FSM illegal transitions, outbox relay.
