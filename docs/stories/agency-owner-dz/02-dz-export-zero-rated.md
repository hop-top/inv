# Story agency-owner-dz-02: DZ → foreign-buyer invoice resolves as zero-rated export

**Persona**: [Agency owner (Algiers, fin+inv local)](../../personas/agency-owner-dz.md)

## Story

As a DZ-16 agency owner billing foreign clients, I want an invoice to a
non-DZ buyer (e.g. FR, US) to resolve as `export` with 0% TVA and the
rendered document to carry an `export = true` flag, so that my accountant
can confirm the foreign-export treatment without inspecting the tax table.

## Acceptance criteria

**Given** a customer `customer_02...` exists with `address.country = "FR"`
**When** I run
```sh
inv invoice draft \
  --customer customer_02... --currency EUR \
  --seller-jurisdiction DZ-16 \
  --line "Strategy workshop:1:5000.00"
```
**Then** the draft persists with `subtotal = "5000.00"` (EUR, 2 decimals).

**Given** the draft above
**When** I issue the invoice
**Then** the resolver determines buyer scope = `export` (different country
per [contracts/tax-resolution.md#step-1--determine-buyer-scope](../../contracts/tax-resolution.md#step-1--determine-buyer-scope)),
applies 0% TVA, sets `tax_total = "0.00"`, sets `total = "5000.00"`, and
the line is flagged `export = true` on the rendered HTML/PDF.

**Given** the same draft but the operator explicitly tagged the line
`tax_category = "zero_rated"` (via API / WS payload) AND a matching
`zero_rated` row exists in the table
**When** the resolver runs
**Then** the row id is recorded in `invoice_lines.tax_rate_ids` for audit
(per [contracts/tax-resolution.md#export](../../contracts/tax-resolution.md)),
even though the applied rate is `0%`. This distinguishes "zero-rated
because export" (no row id) from "zero-rated because configured as such"
(row id recorded).

**Given** the invoice is issued and sent via `link://` to the foreign
client
**When** `fin` later reports `fin.billing.payment.received` for the full
amount
**Then** the invoice transitions to `paid`. (No FX conversion — design
[§2.1](../../architecture/design-spec.md#21-in-scope-v1) is explicit;
the EUR amount is treated as the realised payment.)

## Surfaces touched

- CLI — `inv invoice draft`, `inv invoice issue`, `inv invoice send --to link://`.
- Tax engine — `resolve_tax` on `export` scope.
- Store — `invoices.tax_total = "0.00"`, `invoice_lines.tax_amount = "0.00"`.
- Render — `export = true` flag on the line.
- Bus consumer — `fin.billing.payment.received` for the EUR payment.

## Out of scope for this story

- FX conversion of EUR ↔ DZD — explicitly out of scope at v1.
- Customs / shipping documents — `inv` doesn't generate these.
- VAT-ID validation on the foreign buyer — no `aps` / external service
  call at v1.

## See also

- [contracts/tax-resolution.md#export](../../contracts/tax-resolution.md) — buyer-scope determination + audit semantics.
- [contracts/signed-link-token.md](../../contracts/signed-link-token.md) — delivery via `link://` for the foreign client.
- [agency-owner-dz-01](01-dz-local-tva-reduced.md) — the local-TVA counterpart.
