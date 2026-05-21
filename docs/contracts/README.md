# Contracts

For integrators who need to know **the wire shape can't drift**. Each page
here is a frozen contract — bus payloads, FSM transition tables, signed-link
tokens. Treat them as load-bearing.

| You need to... | Read |
|---|---|
| Know which invoice transitions are legal | [invoice-state-fsm.md](invoice-state-fsm.md) |
| Know which credit-note transitions are legal | [creditnote-state-fsm.md](creditnote-state-fsm.md) |
| Apply tax exactly the way `inv` does | [tax-resolution.md](tax-resolution.md) |
| Subscribe to `inv.billing.*` and parse payloads | [bus-payload-schemas.md](bus-payload-schemas.md) |
| Verify or mint a signed view link | [signed-link-token.md](signed-link-token.md) |

## What "contract" means here

A contract page is **the wire-shape definition for that boundary**. We don't
change it without a major-version bump. Internal refactors are free to move
code around, rename fields, swap implementations — as long as what crosses
the boundary stays the same.

## Provenance

Every contract page cites its source files. When a contract and source
diverge, the source is right and the contract is a bug.
