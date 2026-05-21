# How to configure tax rates and nexus

For an operator who needs to add a jurisdiction, adjust a rate, opt into US
economic nexus, or override the bundled defaults.

`inv` is **not** a tax-law oracle. It applies *configured rates* against a
*configured jurisdiction model*. Rate accuracy and economic-nexus thresholds
are the operator's responsibility — keep the table current.

> See the [tax-resolution contract](../contracts/tax-resolution.md) for the
> exact algorithm `inv` runs against your table.

## Where the table lives

The bundled table ships at [`tax-tables/default.toml`](../../tax-tables/default.toml).
Point `inv.toml` at it (or your own override):

```toml
[tax]
tax_tables = "tax-tables/default.toml"   # or "/etc/inv/my-rates.toml"
```

Relative paths resolve against the config file's parent dir; absolute paths
are used verbatim.

Full TOML schema + every shipped row: [reference/tax-tables.md](../reference/tax-tables.md).

## Add a rate

Append to your table:

```toml
[[rate]]
id = "ca-ns-hst"
jurisdiction = "CA-QC"                              # seller jurisdiction
applies_to_buyer = { country = "CA", region = "NS" }
name = "HST (Nova Scotia)"
category = "standard"
rate = "0.10"                                        # 10%; the federal 5% GST row stacks on top
effective_from = "2010-07-01"
```

`id` is stable and used in audit (`invoice_lines.tax_rate_ids`). Keep it
kebab-case + descriptive.

`applies_to_buyer` filters which buyers this row matches:

- `{ country = "CA" }` — any CA buyer.
- `{ country = "CA", region = "QC" }` — only QC buyers.
- `{ }` — universal (rare; usually only `zero_rated` / `exempt`).

`category`:

- `standard` — applies when the line is `tax_category = standard` (default).
- `reduced` — applies when the line is `tax_category = reduced`.
- `zero_rated` — applies when the line is `tax_category = zero_rated`. Rate
  is `"0.00"` but the row is recorded in the line's `tax_rate_ids` for audit.
- `exempt` — same as `zero_rated` but with different audit semantics (the
  good is **not subject** to tax; vs. zero-rated which is taxable at 0%).

`effective_from` / `effective_to` (optional) bound when the row is active.
The resolver picks the row whose date range covers the invoice's
`occurred_at`.

## Multi-row stacking

A single buyer may match multiple rows (QC seller → QC buyer matches
`ca-qc-gst` AND `ca-qc-qst`). The resolver sums them. To avoid double-counting
(ON HST is 13% which already includes the 5% federal GST), model only the
*provincial slice* in the provincial row — the GST row contributes the
federal slice. See the comments in `tax-tables/default.toml` for the
worked-out CA examples.

## US economic nexus

Delaware sellers (the only US seller shipped at v1) don't auto-collect sales
tax. Each US state's row is gated by a `[nexus.<STATE>]` block:

```toml
[nexus.CA]
enabled = true                # operator opt-in
revenue = "500000.00"         # USD threshold; null disables
txn_count = null              # txn-count threshold; null disables

[nexus.NY]
enabled = true
revenue = "500000.00"
txn_count = 100
```

Rules:

- If `enabled = false` (the shipped default everywhere), the rate is **not**
  applied. The invoice gets `nexus_review = true` so operators can review.
- If `enabled = true`, the rate applies whenever **either** threshold is
  crossed (revenue OR txn_count, never AND).
- `inv` does **not** maintain the running revenue/txn count for you. v1 treats
  `enabled = true` as "the operator has confirmed nexus exists". Future
  versions will track aggregates.

## Reload after edits

The CLI re-reads the table on every invocation, so flag edits take effect on
the next command. For a long-lived `inv server` process, restart the binary —
hot-reload of `tax-tables/*.toml` is deferred.

## Inspect what's loaded

```sh
$ inv tax rates show
```

Outputs every row in the table (table by default; add `--format json` for the
full structure).

```sh
$ curl -H "Authorization: Bearer dev-token" http://127.0.0.1:7400/v1/tax/rates
```

Same data over HTTP. Also reachable via MCP as `inv_tax_rates_show`.

## Disclaimers (design §6.3)

- Defaults are best-effort starting points sourced from public revenue-authority
  pages, dated, and **not legal advice**.
- US locality (city / county / district) sales tax is **out of scope at v1**.
  State-level only. The table shape is forward-compatible — when locality
  lands, an `applies_to_buyer.locality` field will join the existing
  `country` / `region`.
- Currency-conversion is out of scope. Invoices are per-currency; if a CA
  buyer pays a USD-denominated invoice, that's a manual reconciliation step.

## See also

- [Tax-resolution contract](../contracts/tax-resolution.md) — the algorithm
  with all the edge cases reproduced.
- [Tax-tables reference](../reference/tax-tables.md) — the TOML schema +
  every shipped row.
- [Troubleshooting → nexus_review flag set](troubleshooting.md#an-invoice-shows-nexus_review--true)
