# inv

Invoicing-as-a-service: composer + lifecycle, multi-channel core, event-bus
interop with [fin](https://github.com/hop-top/fin).

> Status: early development. `cargo check` is green; functional code lands in
> later tasks (see the [`inv-v1` track](.tlc/tracks/inv-v1/plan.md)).

## What it is

`inv` is a local-first, agent-native invoicing service. It generates invoices,
runs their lifecycle (`draft → issued → sent → viewed → paid/voided/credited`),
schedules reminders, supports recurring billing, and computes destination-aware
sales tax. The same operations are reachable through five channels over a
single command core: CLI, HTTP API, WebSocket, MCP, and event bus.

## Workspace layout

```
inv/
├── bin/
│   └── inv/                  the `inv` binary (thin entrypoint over crates/cli)
└── crates/
    ├── cli/                  CLI channel adapter
    ├── api/                  HTTP API channel adapter (axum)
    ├── ws/                   WebSocket channel adapter
    ├── mcp/                  MCP server channel adapter
    ├── bus/                  event bus types + publisher/consumer
    ├── store/                persistence: sql + blob
    └── core/                 domain types, FSM, tax engine, render, commands
```

## Build

Prerequisites: Rust 1.85+ (workspace edition 2021), cargo.

```sh
make setup    # cargo fetch
make check    # fmt-check + clippy + test
make test     # cargo test --workspace
make lint     # cargo fmt --check + clippy
```

## Design

- [Design spec](https://github.com/jadb/ideacrafterslabs-docs/blob/main/superpowers/specs/2026-05-20-inv-design.md) — scope, architecture, FSM, tax engine, data model
- [Implementation plan](.tlc/tracks/inv-v1/plan.md) — 23 tasks across 6 phases

## License

[MIT](LICENSE).
