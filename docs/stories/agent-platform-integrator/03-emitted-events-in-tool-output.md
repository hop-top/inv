# Story agent-platform-integrator-03: Tool output surfaces emitted bus events to the agent

**Persona**: [Agent platform integrator](../../personas/agent-platform-integrator.md)

## Story

As an engineer wiring an agent that explains its actions back to the user,
I want every command's response to include the list of bus events that
were queued for emission, so that the agent can summarise side-effects
("the invoice was issued and three events were queued for the bus")
without subscribing to the bus separately.

## Acceptance criteria

**Given** an MCP-bound agent has called `inv_invoice_draft` successfully,
producing `invoice_01...` in `state = "draft"`
**When** the agent calls
```json
{
  "method": "tools/call",
  "params": {
    "name": "inv_invoice_issue",
    "arguments": { "invoice_id": "invoice_01..." }
  }
}
```
**Then** the tool response includes an `emitted_events` field — the typed
`Vec<EmittedEvent>` produced by the `issue_invoice` command — containing,
in emission order, the mechanic triplet + the domain event for the
transition:
```
inv.billing.invoice.proposed       (mechanic, sync, veto-able)
inv.billing.invoice.transitioned   (mechanic)
inv.billing.invoice.entered        (mechanic)
inv.billing.invoice.issued         (domain)
```
Each entry carries the topic + payload shape documented in
[contracts/bus-payload-schemas.md](../../contracts/bus-payload-schemas.md).
The agent can serialise this list straight into its tool-result message.

**Given** the same tool call has just landed
**When** the outbox relay drains `invoice_state_history`
**Then** the corresponding bus emissions happen exactly once per row
(at-least-once subscriber semantics — see
[reference/event-bus.md#idempotency--ordering-design-44](../../reference/event-bus.md#idempotency--ordering-design-44)),
and the agent's pre-computed `emitted_events` list matches what
subscribers see on the wire.

**Given** the issue would violate the FSM (e.g. the invoice is already
`issued`)
**When** the tool call lands
**Then** the response is `McpError::Command` wrapping
`CoreError::FsmTransition`, no row is inserted into
`invoice_state_history`, and `emitted_events` is empty/absent — the
agent can detect the failure without inspecting the bus.

## Surfaces touched

- MCP — every `inv_invoice_*`, `inv_creditnote_*`, `inv_schedule_*`,
  `inv_reminder_*` tool returns `EmittedEvent` in its response.
- Commands — `EmittedEvent` is part of the command return type for every
  mutating operation (design [§3.5](../../architecture/design-spec.md#35-cross-cutting-concerns)).
- Outbox relay — separate process, surfaces the same events to bus
  subscribers; the agent's view is *predictive* (the relay drains async).

## Out of scope for this story

- Bus subscribers consuming the same events — subscribers are independent
  consumers; this story is about the agent's in-band visibility.
- Veto subscribers rejecting `.proposed` — the sync veto seam is
  documented but exercising it requires a custom subscriber outside the
  agent.
- Persisting the `emitted_events` list — it's part of the tool response,
  not durably stored as a separate artifact (the durable artifact is
  `invoice_state_history`).

## See also

- [contracts/bus-payload-schemas.md](../../contracts/bus-payload-schemas.md) — payload shapes for every emitted event.
- [reference/event-bus.md](../../reference/event-bus.md) — outbox + at-least-once contract.
- [architecture/design-spec.md §3.5](../../architecture/design-spec.md#35-cross-cutting-concerns) — single-transaction FSM + outbox invariant.
