# Architecture

This section is for contributors. It explains what `inv` is, why each piece
exists, and where to read deeper.

For day-to-day operator tasks, use [`../user/`](../user/). For lookup tables
(every CLI flag, every API route, every bus topic), use
[`../reference/`](../reference/). For frozen wire shapes, use
[`../contracts/`](../contracts/).

## Design spec — the source of truth

[**design-spec.md**](design-spec.md) is the canonical design document. It
covers:

- §1 Purpose + scope
- §3 One core, thin adapters
- §3.4 Invoice lifecycle FSM
- §4 Event bus (emitted + consumed topics; mechanic-event triplet)
- §5 Data model (every table, every column)
- §6 Tax engine (inference algorithm, file shape, disclaimers)
- §7 Delivery channels (`file://`, `stdout`, `bus://`, `webhook://`, `link://`)
- §8 Recurring schedules
- §9 Reminders
- §10 Channel-specific surfaces (the cross-channel operation matrix)
- §11 Configuration
- §13 References (kit + fin + uri-poly)

When this docs tree summarises the spec, it links back into the relevant
section rather than duplicating the prose. Treat the spec as authoritative.

## Workspace at a glance

```
inv/
├── bin/inv/                     thin binary entrypoint
└── crates/
    ├── core/                    domain types, FSM, tax, render
    ├── store/                   sqlx + blob; migrations + repos
    ├── commands/                one-function-per-operation core
    ├── bus/                     event types, outbox relay, inbox dedup, consumer
    ├── cli/                     clap adapter
    ├── api/                     axum adapter (HTTP + signed-link view)
    ├── ws/                      WebSocket adapter (frame protocol + subscriptions)
    └── mcp/                     MCP server adapter (rmcp; stdio transport)
```

Adding a new channel = a new crate that depends on `crates/commands`. Zero
changes to the core. The crate names in source are `inv-core`, `inv-store`,
`inv-commands`, `inv-bus`, `inv-cli`, `inv-api`, `inv-ws`, `inv-mcp`
(workspace path → crate name mapping).

## Cross-cutting invariants (design §3.5)

Every command:

1. Validates input up front.
2. Checks `idempotency_key` BEFORE mutating — replays return the original
   output.
3. Opens a single DB transaction encompassing the mutation + the
   `invoice_state_history` insert (the history row doubles as the outbox).
4. Returns the list of bus events it *would* emit; the relay publishes them
   when `published_at` is set.

## Adjacent reading

- [Invoice FSM contract](../contracts/invoice-state-fsm.md) — every legal transition.
- [Credit-note FSM contract](../contracts/creditnote-state-fsm.md).
- [Tax-resolution contract](../contracts/tax-resolution.md) — the algorithm
  from spec §6.1 reproduced as a step-by-step contract.
- [Bus payload schemas](../contracts/bus-payload-schemas.md) — every emitted
  and consumed event payload.
- [Signed-link token format](../contracts/signed-link-token.md).
