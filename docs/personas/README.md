# Personas

Stack-aware reading guides. Each persona below describes a **role** that runs
`inv` as part of a hop-top toolchain — typically with `fin` as the ledger,
sometimes with `aps` as the agent runtime, sometimes with an MCP-capable
agent app on top.

These are not pure-`inv` personas. The composition is the point. `inv` owns
the invoice document and FSM; `fin` owns money movement; the two communicate
over the bus (`inv.billing.*` ↔ `fin.billing.*`). The personas here describe
who is in front of that stack and what they're trying to do.

## Why personas in inv at v1-alpha (deliberate)

The earlier draft of this README hedged that personas might belong in
`hop-top/aps`. That hedge is **overridden**. v1-alpha personas live here
because:

- The first operators meeting `inv` meet it through one of its five surfaces
  (CLI, HTTP, WS, MCP, bus). They need reading paths tuned to their surface
  and their stack composition — not generic role descriptions.
- `aps` does not yet ship a personas tree. Putting personas there before
  `aps` is ready to host them would block the value here.
- Cross-tool personas (anything that mentions both `fin` and `inv`, or an
  MCP-embedded agent built on top) belong wherever the reader first lands.
  Today that's `inv`.

## Migration intent

When `aps` ships its own personas tree, the cross-tool personas may move
there. `inv`-specific reading paths (the surface map, the per-persona
"reading path" section that points at this doc tree) stay behind. The migrated
files will leave forwarding stubs.

## The persona files

| Persona | One-line hook |
|---|---|
| [Freelancer (QC, fin+inv local)](freelancer-qc.md) | Solo consultant in Quebec billing US + CA clients from a laptop. |
| [SaaS biller (DE, fin+inv as a service)](saas-biller-de.md) | Small Berlin team running `inv` behind their own webhook integration for EU + US customers. |
| [Agent platform integrator](agent-platform-integrator.md) | Engineer embedding `inv` via MCP as a tool in an agent app. |
| [Agency owner (DZ-16, fin+inv local)](agency-owner-dz.md) | Algiers agency billing DZ-local + DZ-export clients with recurring retainers. |

Each file follows the same shape: context, what they want, what they don't
need, known v1 gaps, and an ordered reading path that should get them
productive in ~30 min.

## See also

- [user/](../user/) — task-oriented guides referenced from every persona.
- [reference/](../reference/) — surface-by-surface lookup tables.
- [contracts/](../contracts/) — frozen wire shapes.
- [stories/](../stories/) — user stories grouped by persona.
- [fin's personas tree](https://github.com/hop-top/fin/tree/main/docs/personas) — prior art for shape + tone.
