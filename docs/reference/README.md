# Reference

Lookup tables for every public surface. Pick by what you're holding:

| You have... | Read |
|---|---|
| A shell prompt | [cli.md](cli.md) |
| An HTTP client | [api.md](api.md) |
| A WebSocket client | [ws.md](ws.md) |
| An MCP agent | [mcp.md](mcp.md) |
| A bus subscriber | [event-bus.md](event-bus.md) |
| A `tax-tables/*.toml` file | [tax-tables.md](tax-tables.md) |
| An `inv.toml` file | [config.md](config.md) |

These pages are **lookup tables**, not tutorials. For the "what do I run to
get a paid invoice?" path, see [`../user/`](../user/).

## What this section is NOT

- It is not the design spec — that's [architecture/design-spec.md](../architecture/design-spec.md).
- It is not the FSM contract — that's [contracts/invoice-state-fsm.md](../contracts/invoice-state-fsm.md).
- It is not the tax-resolution algorithm — that's
  [contracts/tax-resolution.md](../contracts/tax-resolution.md).

Reference pages link out to those when context is needed; they don't restate
them.
