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

## Local setup

Opt into repo-local git hooks (pre-push runs `cargo fmt --check`):

```sh
git config core.hooksPath .githooks
```

See [`.githooks/README.md`](.githooks/README.md) for details and bypass options.

## Design

[`docs/`](docs/) is organised by reader intent — pick the doc that matches
what you want to do.

I want to...

- **Install + run my first invoice** → [docs/user/tutorial-getting-started.md](docs/user/tutorial-getting-started.md)
- **Look up a CLI flag, API route, MCP tool, or bus topic** → [docs/reference/](docs/reference/)
- **Understand the FSM, tax algorithm, or signed-link format** → [docs/contracts/](docs/contracts/)
- **Read the full design** → [docs/architecture/design-spec.md](docs/architecture/design-spec.md)
- **See the implementation plan** → [`.tlc/tracks/inv-v1/plan.md`](.tlc/tracks/inv-v1/plan.md) — 23 tasks across 6 phases

## License

[MIT](LICENSE).
