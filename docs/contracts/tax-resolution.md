# Tax resolution

The algorithm `inv` runs against your tax table. Reproduced verbatim from
design [§6.1](../architecture/design-spec.md) with the edge cases inlined.

Source: [`crates/core/src/tax/resolver.rs`](../../crates/core/src/tax/resolver.rs)
and [`crates/core/src/tax/nexus.rs`](../../crates/core/src/tax/nexus.rs).

## Inputs

```rust
fn resolve_tax(
    seller_jur: Jurisdiction,           // CA-QC | US-DE | DZ-16
    buyer:      CustomerAddress,        // country + region + city
    line:       &Line,                  // carries tax_category override
    table:      &TaxTable,              // tax-tables/default.toml + overlays
    nexus:      &NexusConfig,
    at:         DateTime<Utc>,
) -> Result<ResolvedTax, TaxError>
```

## Step 1 — determine buyer scope

| Condition | Scope |
|---|---|
| Same country + same region | `domestic_local` |
| Same country + different region | `domestic_other` |
| Different country | `export` |

## Step 2 — pick applicable rate(s)

### `domestic_local`

Look up `(seller_jur, buyer_region, line.tax_category, at)` in the rate table.
**Multiple rates may apply** — e.g. QC → QC matches `ca-qc-gst` + `ca-qc-qst`
(GST 5% + QST 9.975% = 14.975% effective). The resolver returns the set; the
caller sums.

### `domestic_other`

Three sub-cases depending on the seller country.

**CA seller, CA buyer (non-QC):** destination-based. The table has a row per
province for the QC seller — pick the buyer's province row. Stacks on top of
the federal `ca-qc-gst` row (which is `country = "CA"` with no region filter,
so it matches every CA buyer regardless of region).

**US seller (US-DE), US buyer (non-DE):** **nexus check**.

1. Lookup `nexus[buyer_state]` in `[nexus.<STATE>]`.
2. If `nexus[buyer_state].enabled == false`: **0 % applied**; invoice gets
   `nexus_review = true` so operators can investigate.
3. If `nexus[buyer_state].enabled == true`: check thresholds.
   - If `revenue` is set AND seller's revenue in that state ≥ threshold → apply.
   - OR if `txn_count` is set AND seller's txn count ≥ threshold → apply.
   - Otherwise 0 % + `nexus_review = true`.

> **Important:** `inv` does NOT maintain the running revenue/txn count for
> you at v1. The operator must opt in (`enabled = true`) once they're confident
> the threshold is met. Future versions will track aggregates.

**DZ seller (DZ-16), DZ buyer (non-Algiers):** treat as `domestic_local`. TVA
is national in Algeria — same row regardless of buyer wilaya.

### `export`

0 % applied; line flagged `export = true` on the rendered document. If the
operator explicitly tagged the line `tax_category = "zero_rated"` AND there's
a matching `zero_rated` row in the table, the row id is recorded in
`invoice_lines.tax_rate_ids` for audit.

## Step 3 — apply line tax category

| Line `tax_category` | Resolver behaviour |
|---|---|
| `standard` (default) | Use the `standard` rows for the matched jurisdiction set. |
| `reduced` | Use the `reduced` rows if any exist. **Fallback** to `standard` with a warning if no `reduced` row covers the buyer scope. |
| `zero_rated` | 0 %; rate id recorded in `tax_rate_ids` for audit. |
| `exempt` | 0 %; recorded as exempt (different audit semantics from `zero_rated` — zero-rated is taxable at 0%; exempt is not taxable). |

## Step 4 — compute amount

Compute in the **line's currency** at the invoice's `occurred_at`. **No FX.**

Banker's rounding (round-half-to-even) to the currency's minor-unit scale:

| Currency | Scale |
|---|---|
| USD | 2 decimals |
| CAD | 2 decimals |
| DZD | 0 decimals |

The sum across all matched rows for one line is rounded once at the end —
not per-row.

## Examples

### QC seller → QC buyer, line $1000 standard

- Matches `ca-qc-gst` (5%) + `ca-qc-qst` (9.975%).
- Tax = $1000 × (0.05 + 0.09975) = $149.75.

### QC seller → ON buyer, line $1000 standard

- Matches `ca-qc-gst` (5%) + `ca-on-hst` (8%).
- Tax = $1000 × 0.13 = $130.00. **(matches the published HST-ON rate of
  13%)**

### QC seller → AB buyer, line $1000 standard

- Matches `ca-qc-gst` (5%) + `ca-ab-no-pst` (exempt — 0%).
- Tax = $1000 × 0.05 = $50.00.

### US-DE seller → US-CA buyer, nexus disabled (the default)

- `[nexus.CA].enabled = false`.
- Tax = 0; `invoices.nexus_review = true`.

### US-DE seller → US-CA buyer, nexus enabled

- `[nexus.CA].enabled = true`, operator has confirmed nexus.
- Matches `us-ca-sales` (7.25%).
- Tax = $1000 × 0.0725 = $72.50.

### DZ-16 seller → DZ buyer, line reduced

- Matches `dz-tva-reduced` (9%).
- Tax = (line_total in DZD) × 0.09, rounded to **0 decimals**.

### CA-QC seller → US buyer, any line

- Different country → `export`.
- Tax = 0.

## Shortcuts and disclaimers (design §6.3 — explicit)

- **`inv` is a tax calculator from configured rates, not a tax-law oracle.**
  Rate accuracy and economic-nexus thresholds are the operator's responsibility.
- Defaults shipped in `tax-tables/default.toml` are best-effort starting
  points sourced from public revenue-authority pages, dated, and **not legal
  advice**. Keep them current.
- **US locality (city / county / district) sales tax is out of scope at
  v1.** State-level only. The table shape is forward-compatible
  (`applies_to_buyer.locality` can be added later without migration).
- **No automatic nexus tracking at v1.** Operators opt in per state once
  they're confident the threshold is met. The flag is `enabled`; presence
  signals "we have nexus".
- **Reduced-rate fallback to standard is silent** — the warning surfaces in
  logs (`tracing` at WARN level) but doesn't fail the call. Audit
  `invoice_lines.tax_rate_ids` to see which rate actually applied.

## See also

- [tax-tables.md](../reference/tax-tables.md) — file schema + every shipped row.
- [how-to-tax-config.md](../user/how-to-tax-config.md) — operator workflow.
- Design spec [§6](../architecture/design-spec.md) — full tax engine design.
