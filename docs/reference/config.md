# Config reference

Schema for `inv.toml`. Authored from
[`crates/cli/src/config.rs`](../../crates/cli/src/config.rs) and design §11.

## File location

In order of precedence:

1. `--config <path>` flag.
2. `$XDG_CONFIG_HOME/inv/config.toml`.
3. `$HOME/.config/inv/config.toml`.
4. **No file** — `inv` falls back to in-memory sqlite + the bundled
   `tax-tables/default.toml`.

## Sections

```toml
[storage]
backend = "sqlite"                       # sqlite | postgres | tidb
dsn     = "file:./inv.sqlite"
blob    = "blob://local/./inv-blobs"     # kit blob URI (S3 also supported)

[bus]
topics_in = [                            # subscriptions
  "fin.billing.charge.created",
  "fin.billing.payment.received",
  "fin.billing.payment.refunded",
]
[bus.remap]                              # rename inbound topics
"finance.charges.created" = "fin.billing.charge.created"

[tax]
tax_tables                    = "tax-tables/default.toml"   # or absolute path
default_seller_jurisdiction   = "CA-QC"
rounding                      = "bankers"                   # only "bankers" at v1

[render]
template_path = "templates/default"
pdf_engine    = "wkhtmltopdf"            # selected via Cargo feature; "weasyprint" | "typst"

[link]
public_base_url = "https://invoices.example.com"
signing_key     = "${INV_LINK_SIGNING_KEY}"
token_ttl       = "30d"

[ticker]
enabled            = true
schedules_interval = "24h"
reminders_interval = "5m"
overdue_interval   = "1h"

[server]
api_listen = "127.0.0.1:7400"
ws_listen  = "127.0.0.1:7401"
mcp_stdio  = true
```

## Fields

### `[storage]`

| Field | Default | Notes |
|---|---|---|
| `backend` | `"sqlite"` | One of `sqlite | postgres | tidb`. Postgres / tidb are compile-gated behind Cargo features. |
| `dsn` | `"sqlite::memory:"` | sqlx-style DSN. |
| `blob` | `"blob://local/./inv-blobs"` | kit blob URI. Local FS by default; S3-compatible via `blob://s3/...`. |

### `[tax]`

| Field | Default | Notes |
|---|---|---|
| `tax_tables` | `"tax-tables/default.toml"` | Path (or list of paths for overlay order). Relative paths resolve against the config's parent dir. |
| `default_seller_jurisdiction` | `"CA-QC"` | Used when an inbound payload doesn't specify. One of `CA-QC | US-DE | DZ-16`. |
| `rounding` | `"bankers"` | Only `bankers` (round-half-to-even) at v1. |

### `[bus]`

| Field | Default | Notes |
|---|---|---|
| `topics_in` | `[]` (empty) | Inbound topics the consumer subscribes to. |
| `[bus.remap]` | (empty) | Operator-side renaming. Maps an inbound topic name to a canonical `inv` topic, so deployments can subscribe to whatever `fin` (or any publisher) ships without recompiling. |

### `[render]`

| Field | Default | Notes |
|---|---|---|
| `template_path` | `"templates/default"` | Path to a directory containing `invoice.html` + assets. |
| `pdf_engine` | (Cargo-feature default) | One of `wkhtmltopdf | weasyprint | typst`. Compile-time selection: feature `pdf-wkhtmltopdf` / `pdf-weasyprint` / `pdf-typst`. |

### `[link]`

| Field | Default | Notes |
|---|---|---|
| `public_base_url` | (none) | Base URL used to mint signed-link URLs. `inv server`'s `/v/{token}` route serves them. |
| `signing_key` | (env) | HMAC-SHA256 key. Use `${INV_LINK_SIGNING_KEY}` to read from env. |
| `token_ttl` | `"30d"` | Token expiry (e.g. `30d`, `12h`). |

See [contracts/signed-link-token.md](../contracts/signed-link-token.md) for
the token format.

### `[ticker]`

| Field | Default | Notes |
|---|---|---|
| `enabled` | `true` | Disable to run `inv` as a pure on-demand library. |
| `schedules_interval` | `"24h"` | How often to materialise recurring schedules. |
| `reminders_interval` | `"5m"` | How often to dispatch reminders. |
| `overdue_interval` | `"1h"` | How often to flag overdue invoices. |

CLI flags on `inv server` override these (e.g. `--schedules-interval-secs`).

### `[server]`

| Field | Default | Notes |
|---|---|---|
| `api_listen` | `"127.0.0.1:7400"` | HTTP + WS listen address (one listener). |
| `ws_listen` | `"127.0.0.1:7401"` | Reserved for future split-listener mode; unused at v1. |
| `mcp_stdio` | `true` | Enable MCP over stdio when booting via `inv server --mcp`. |

## Env-var overrides

Anywhere `${VAR}` appears in a string value, `inv` substitutes from the
process environment at config-load time. Useful for secrets:

```toml
[link]
signing_key = "${INV_LINK_SIGNING_KEY}"

[bus]
topics_in = ["${BUS_TOPIC_CHARGE_CREATED}"]
```

Missing env vars are kept literal (`${VAR}` is treated as a literal string).
Quote env values that might contain special chars.

## Minimal config

```toml
[storage]
dsn = "sqlite:./inv.sqlite"
```

Everything else defaults to the design-spec values.

## Subset accepted today

The v1 CLI loader (`crates/cli/src/config.rs`) only consumes `[storage]`,
`[tax]`, and `[bus]` keys; the rest are **accepted and ignored** so operators
can keep one file across adapters. The full surface lands as more adapters
wire up (`inv server` reads `[ticker]` and `[server]`; the public-link route
will read `[link]`).
