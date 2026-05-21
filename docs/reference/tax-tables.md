# Tax tables reference

TOML schema + every row shipped in [`tax-tables/default.toml`](../../tax-tables/default.toml).

## File shape (design §6.2)

```toml
[[rate]]
id               = "<kebab-id>"             # stable; recorded in invoice_lines.tax_rate_ids
jurisdiction     = "CA-QC" | "US-DE" | "DZ-16"
applies_to_buyer = { country = "<ISO>", region = "<ISO subdivision>" }
                                            # both optional;
                                            # `{}` = universal match
name             = "<human label>"
category         = "standard" | "reduced" | "zero_rated" | "exempt"
rate             = "<decimal fraction>"     # "0.05" = 5%
effective_from   = "YYYY-MM-DD"
effective_to     = "YYYY-MM-DD"             # optional, inclusive

[nexus.<STATE>]                             # e.g. [nexus.CA]
enabled          = false                    # operator must opt in
revenue          = "<decimal>"              # USD threshold; null disables
txn_count        = <int>                    # txn-count threshold; null disables
```

`id` is the stable identifier — keep it kebab-case + descriptive. The id is
written to `invoice_lines.tax_rate_ids` for audit.

`category` determines which line tax-categories the row matches. Lines have a
`tax_category` (default `standard`); the resolver picks rows whose `category`
matches the line's, with fallback `reduced → standard` (and a warning) when
no `reduced` row exists.

`applies_to_buyer` filters which buyers the row matches. Both `country` and
`region` are optional:

- `{ country = "CA" }` — any CA buyer (federal-scope rate).
- `{ country = "CA", region = "QC" }` — only QC buyers.
- `{ }` — universal (zero-rated exports, etc.).

`effective_from` / `effective_to` (optional) bound when the row is active.
The resolver picks rows whose date range covers the invoice's
`occurred_at`. Multiple rows can match a single line (e.g. QC seller → QC
buyer matches `ca-qc-gst` AND `ca-qc-qst` — the resolver sums them).

## Default table — every shipped row

### CA-QC seller

| ID | Buyer match | Name | Category | Rate |
|---|---|---|---|---|
| `ca-qc-gst` | CA | GST | standard | 0.05 |
| `ca-qc-qst` | CA + QC | QST | standard | 0.09975 |
| `ca-on-hst` | CA + ON | HST (Ontario provincial slice) | standard | 0.08 |
| `ca-bc-pst` | CA + BC | PST (British Columbia) | standard | 0.07 |
| `ca-ab-no-pst` | CA + AB | Alberta (GST only, no PST) | exempt | 0.00 |
| `ca-qc-gst-zero-foods` | CA | GST zero-rated (basic groceries) | zero_rated | 0.00 |

Modeling note: HST is "harmonised" — federal GST + provincial component
rolled into one rate. To avoid double-counting, the table models only each
province's **provincial slice**, and the federal `ca-qc-gst` row (`country =
"CA"`, no region) contributes the federal 5%. Stacked:

- CA-QC → ON buyer = `ca-qc-gst` (5%) + `ca-on-hst` (8%) = **13%** ✓ (HST-ON)
- CA-QC → BC buyer = `ca-qc-gst` (5%) + `ca-bc-pst` (7%) = **12%** (GST + PST)
- CA-QC → AB buyer = `ca-qc-gst` (5%) + `ca-ab-no-pst` (exempt) = **5%** (GST only)

### DZ-16 seller

| ID | Buyer match | Name | Category | Rate |
|---|---|---|---|---|
| `dz-tva-standard` | DZ | TVA | standard | 0.19 |
| `dz-tva-reduced` | DZ | TVA réduite | reduced | 0.09 |
| `dz-tva-export-zero` | (universal) | TVA zero-rated (exports) | zero_rated | 0.00 |

TVA is national in Algeria — same row applies regardless of buyer wilaya.

### US-DE seller

Delaware has no general sales tax. Out-of-state sales rely on the
destination state's **economic-nexus regime**. Only rows whose matching
`[nexus.<STATE>]` block is `enabled = true` AND whose threshold the seller
has crossed are applied.

| ID | Buyer match | Name | Category | Rate |
|---|---|---|---|---|
| `us-ca-sales` | US + CA | California State Sales Tax | standard | 0.0725 |
| `us-ny-sales` | US + NY | New York State Sales Tax | standard | 0.04 |

Rates are statewide bases only — **not** including district/county/city
addenda. US locality tax is out of scope at v1 (see design §2.2).

### US economic-nexus thresholds

All `enabled = false` in the shipped defaults — operators opt in once they're
confident nexus exists.

| State | Enabled | Revenue threshold | Txn count |
|---|---|---|---|
| CA | false | 500000 | (null) |
| NY | false | 500000 | 100 |
| TX | false | 500000 | (null) |
| WA | false | 100000 | (null) |

## Override the bundled table

Point `inv.toml` at your file:

```toml
[tax]
tax_tables = "/etc/inv/my-rates.toml"
```

Relative paths resolve against the config file's parent dir; absolute paths
are used verbatim. The CLI re-reads on every invocation; `inv server` re-reads
on restart (hot-reload of the table is deferred).

## Disclaimers (design §6.3)

- `inv` is a tax **calculator** from configured rates, **not a tax-law
  oracle**. Rate accuracy and nexus thresholds are the operator's responsibility.
- Defaults are best-effort starting points sourced from public revenue-authority
  pages, dated, and **not legal advice**.
- US locality (city / county / district) sales tax is **out of scope** at v1.
  State-level only. The table shape is forward-compatible — `applies_to_buyer.locality`
  can be added later without migrating existing rows.

## See also

- [Tax-resolution contract](../contracts/tax-resolution.md) — the algorithm.
- [How-to: configure tax](../user/how-to-tax-config.md) — operator workflow.
