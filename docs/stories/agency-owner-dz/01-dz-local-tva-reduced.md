# Story agency-owner-dz-01: DZ → DZ invoice resolves TVA with a reduced-rate line

**Persona**: [Agency owner (Algiers, fin+inv local)](../../personas/agency-owner-dz.md)

## Story

As a DZ-16 agency owner, I want to issue a DZD invoice to a DZ customer with
one standard-rated line and one reduced-rate line, and have `inv` resolve
TVA at 19% on the first and 9% on the second, rounded to DZD's 0-decimal
scale, so that the rendered total matches my accountant's expectation.

## Acceptance criteria

**Given** a customer `customer_01...` exists with `address.country = "DZ"`
and `address.region = "16"`, and the loaded tax table contains the
`dz-tva-standard` (19%) and `dz-tva-reduced` (9%) rows shown in
[contracts/tax-resolution.md](../../contracts/tax-resolution.md)
**When** I run
```sh
inv invoice draft \
  --customer customer_01... --currency DZD \
  --seller-jurisdiction DZ-16 \
  --line "Consulting:1:100000" \
  --line "Educational material:1:50000"
```
**Then** the draft persists with `subtotal = "150000"` (DZD, 0 decimals).
At draft time, `tax_total = "0"` — tax is frozen at issue per
[contracts/invoice-state-fsm.md](../../contracts/invoice-state-fsm.md).

**Given** the draft above, and the second line is marked `tax_category =
"reduced"` (via HTTP / WS payload — the CLI sets `standard` by default; per
[user/how-to-issue.md](../../user/how-to-issue.md), per-line categories use
the API or WS payload at v1)
**When** I issue the invoice
**Then** the resolver matches:
- Line 1 (`standard`): `dz-tva-standard` → 100000 × 0.19 = 19000.
- Line 2 (`reduced`): `dz-tva-reduced` → 50000 × 0.09 = 4500.

`tax_total = "23500"`, `total = "173500"`, both rounded to 0 decimals
(banker's rounding to DZD's minor-unit scale). The
`invoice_lines.tax_rate_ids` JSON for each line records which row id was
applied.

**Given** a third line is tagged `tax_category = "reduced"` but the table
does NOT contain a matching reduced row for the buyer scope
**When** the resolver runs
**Then** it **falls back to `standard` with a WARN-level trace log** (per
[contracts/tax-resolution.md#step-3--apply-line-tax-category](../../contracts/tax-resolution.md#step-3--apply-line-tax-category))
and the call succeeds — silent fallback is documented behaviour, not a
failure mode.

## Surfaces touched

- CLI — `inv invoice draft`, `inv invoice issue`.
- HTTP API / WS — required at v1 for per-line `tax_category = "reduced"`.
- Tax engine — `resolve_tax` (`crates/core/src/tax/resolver.rs`) on
  `domestic_local` scope.
- Store — `invoices.tax_total`, `invoice_lines.tax_rate_ids`,
  `invoice_lines.tax_amount`.
- Render — DZD totals with 0 decimals.

## Out of scope for this story

- DZ city / wilaya local tax (none modelled at v1 — DZ TVA is national;
  see [contracts/tax-resolution.md](../../contracts/tax-resolution.md)).
- Foreign-buyer zero rating (covered by [agency-owner-dz-02](02-dz-export-zero-rated.md)).
- Recurring retainer (covered separately by the schedule flow that mirrors
  [freelancer-qc-02](../freelancer-qc/02-monthly-retainer-schedule.md)).
- Per-line `tax_category` via CLI — at v1 only `standard` is settable from
  the CLI; richer payloads need HTTP / WS / MCP.

## See also

- [user/how-to-tax-config.md](../../user/how-to-tax-config.md) — confirm DZ rows are loaded.
- [reference/tax-tables.md](../../reference/tax-tables.md) — full TOML schema.
- [contracts/tax-resolution.md](../../contracts/tax-resolution.md) — worked DZ examples.
