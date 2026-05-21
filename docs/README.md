# inv documentation

`inv` is a local-first, agent-native invoicing service. It owns the invoice
document and its lifecycle (`draft → issued → sent → viewed → paid | voided |
credited`), schedules reminders, materialises recurring billing, and computes
destination-aware sales tax for QC / DE / DZ-16 sellers across US / CA / DZ
markets. The same operation set is reachable through five channels — CLI,
HTTP API, WebSocket, MCP, and event bus — over a single command core.

This tree is organised by **reader intent**: pick the doc that matches what
you want to do.

## I want to...

| Intent | Read |
|---|---|
| Install + run my first invoice in 15 minutes | [user/tutorial-getting-started.md](user/tutorial-getting-started.md) |
| Draft, issue, and send an invoice (CLI / API / MCP) | [user/how-to-issue.md](user/how-to-issue.md) |
| Bill on a recurring schedule | [user/how-to-recurring.md](user/how-to-recurring.md) |
| Configure tax rates + nexus thresholds | [user/how-to-tax-config.md](user/how-to-tax-config.md) |
| Fix something that broke | [user/troubleshooting.md](user/troubleshooting.md) |
| Look up a CLI flag | [reference/cli.md](reference/cli.md) |
| Look up an HTTP route | [reference/api.md](reference/api.md) |
| Look up a WebSocket op | [reference/ws.md](reference/ws.md) |
| Look up an MCP tool | [reference/mcp.md](reference/mcp.md) |
| Look up a bus topic | [reference/event-bus.md](reference/event-bus.md) |
| Edit a tax table | [reference/tax-tables.md](reference/tax-tables.md) |
| Edit a config file | [reference/config.md](reference/config.md) |
| Understand the invoice FSM (every legal transition) | [contracts/invoice-state-fsm.md](contracts/invoice-state-fsm.md) |
| Understand the credit-note FSM | [contracts/creditnote-state-fsm.md](contracts/creditnote-state-fsm.md) |
| Understand how tax is resolved | [contracts/tax-resolution.md](contracts/tax-resolution.md) |
| Build something that subscribes to `inv.billing.*` | [contracts/bus-payload-schemas.md](contracts/bus-payload-schemas.md) |
| Verify or mint a signed view link | [contracts/signed-link-token.md](contracts/signed-link-token.md) |
| Read the full design spec | [architecture/design-spec.md](architecture/design-spec.md) |
| Find a persona to read with | [personas/README.md](personas/README.md) |

## Map

```
docs/
├── README.md             ← you are here
├── architecture/         design spec + high-level overview
├── user/                 task-oriented guides (do-this-to-get-that)
├── reference/            surface-by-surface lookup tables (every CLI flag, API route, …)
├── contracts/            frozen wire shapes + FSM tables (treat as load-bearing)
└── personas/             reading guides by audience (placeholder at v1)
```

Diátaxis-shaped: tutorials and how-tos live under [`user/`](user/); reference
material under [`reference/`](reference/); explanation + contracts under
[`architecture/`](architecture/) and [`contracts/`](contracts/).

## Status

`inv` is at v0.1.0 / early development. The design spec is locked; functional
code lands through the
[`inv-v1` track](../.tlc/tracks/inv-v1/plan.md). When this doc tree says "the
command does X", it reflects the source on `hops/main` at the moment the doc
was last updated — when in doubt, check the source path each page cites at the
top.
