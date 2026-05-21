# inv — Invoicing-as-a-Service (design spec)

Date: 2026-05-20
Owner: jad
Status: design — awaiting implementation plan
Target repo: `hop-top/inv`
Companion of: `hop-top/fin`
Scaffolded with: `hop-top/kit` (Rust template)

---

## 1. Purpose

`inv` is a local-first, agent-native invoicing service. It generates invoices,
runs their lifecycle (draft → issued → sent → viewed → paid/voided/credited),
schedules reminders, supports recurring billing, and computes destination-aware
sales tax. It exposes the same operations through five channels — CLI, HTTP API,
WebSocket, MCP, event bus — over a single command core.

`inv` complements `fin` (the ledger) by owning the invoice document and its
lifecycle. `fin` continues to own postings and money movement. The two
communicate through `hop-top/kit`'s event bus over a shared `billing` category.

## 2. Scope

### 2.1 In scope (v1)

| Area | Decision |
|---|---|
| Composer + lifecycle | Stateful invoices and credit notes with a typed FSM |
| Multi-channel core | CLI, HTTP API, WebSocket, MCP, bus consumer — one core, thin adapters |
| Document rendering | Bundled HTML/PDF template; runtime-configurable template path |
| Persistence | kit `sqlstore` (sqlite default, pluggable); kit `blob` for PDFs |
| Event bus | `inv.billing.*` emit; consumes `fin.billing.{charge.created, payment.received, payment.refunded}` |
| Tax | Configurable tax tables + jurisdiction inference (sellers QC / DE / DZ-16; markets US / CA / DZ); line-item tax categories; US economic-nexus flags |
| Multi-currency | Per-invoice ISO 4217 currency; no FX conversion (option A) |
| Recurring billing | Schedule-driven invoice materialisation; no proration, no plan catalog (option A) |
| Delivery channels | `file://`, `stdout`, `bus://`, `webhook://`, `link://` (signed shareable URL) |
| Money type | `rust_decimal::Decimal` |
| Entity IDs | TypeID (Jetify v0.3) — `invoice_01J...`, `customer_01J...`, etc. Wraps `mti` crate behind a facade until `hop_top_kit::id` lands (tracked: `hop-top/poly-kit#id-typeid`). |
| Entity refs on the wire | `hop-top/poly-uri` URI form, scheme `inv`, namespace_segments=1, entity-type namespace: `inv://invoice/invoice_01J...`, `inv://creditnote/creditnote_01J...`, `inv://schedule/schedule_01J...`. Typeid prefix retained inside the id segment. Bus payloads + signed-link tokens emit URIs, not bare typeids. Action routes (`?action=invoice.issue`) deferred. |
| Audit | Bus events + dedicated `invoice_state_history` table (doubles as transactional outbox) |
| Idempotency | Caller-supplied key on every command; `bus_inbox` table for bus event de-dup |
| Rounding | Banker's (round-half-to-even) |
| Config format | TOML |
| Storage backend | kit `sqlstore` (sqlite default; postgres / tidb selectable in config) |

### 2.2 Out of scope (v1)

- Payment service provider integrations (Stripe et al.). Payments are reported back via `fin.billing.payment.received`.
- SMTP / SMS delivery. The bundled delivery channels are file / stdout / bus / webhook / signed link.
- Customer / contact management. `inv` stores `customer_id` as an opaque external reference; CRM is a separate tool.
- FX conversion or multi-currency settlement (deferred B/C of multi-currency).
- Proration, plan catalogs, usage metering (deferred B/C of recurring).
- US local sales tax (city / county / district). State-level only at v1.
- Tax engines for jurisdictions outside QC, DE, and DZ-16; markets outside US, CA, DZ.

### 2.3 Non-goals

- `inv` is not a tax-law oracle. It applies *configured rates* against a *configured jurisdiction model*. Rate accuracy and economic-nexus thresholds are the operator's responsibility.
- `inv` does not move money. Payments and refunds are events from `fin` (or any source publishing the same topics).

## 3. Architecture

### 3.1 One core, thin adapters

Every operation (`draft`, `issue`, `send`, `mark_paid`, `void`, `remind`, `credit`, `schedule_create`, etc.) is defined exactly once in `inv-core/commands/` as an async Rust function taking a typed input and returning a typed output. Each channel adapter does three things only:

1. Decode the channel-native request into the command's input type.
2. Call the command function.
3. Encode the result into the channel-native response.

Invariants (FSM guards, idempotency, tax calculation, validation, audit) live in the command layer. Adapters cannot bypass them — they do not have direct access to the database or the bus.

### 3.2 Workspace layout

```
inv/
├─ Cargo.toml                  # workspace
├─ crates/
│  ├─ inv-core/                # commands, domain types, FSM facade, tax, schedules, render
│  │  ├─ domain/
│  │  │  ├─ invoice/           # Invoice, Line, Tax, State; FSM facade + statig adapter
│  │  │  ├─ creditnote/
│  │  │  ├─ schedule/
│  │  │  └─ customer/
│  │  ├─ tax/                  # tables, jurisdiction inference, nexus rules
│  │  ├─ render/               # HTML → PDF; bundled template; configurable template path
│  │  └─ commands/             # one function per operation
│  ├─ inv-bus/                 # bus event types; kit bus integration; outbox relay; inbox dedup
│  ├─ inv-store/               # kit sqlstore wiring; migrations; kit blob wiring
│  ├─ inv-cli/                 # clap adapter
│  ├─ inv-api/                 # axum adapter
│  ├─ inv-ws/                  # tokio-tungstenite adapter
│  ├─ inv-mcp/                 # MCP server adapter
│  └─ inv-server/              # binary; composes everything
├─ templates/
│  └─ default/                 # bundled invoice template (HTML + CSS)
├─ tax-tables/
│  └─ default.toml             # rates per jurisdiction, nexus thresholds
├─ docs/
└─ tests/
```

Adding a new channel = new crate that depends on `inv-core`. Zero changes to the core.

### 3.3 FSM facade

Rust has no in-tree equivalent of kit's `core/stage`. The most active community state-machine crates are `statig` (hierarchical, async-aware, derive-based) and `rust-fsm` (flat, declarative-macro). v1 ships a facade that wraps `statig` behind an `inv-core` API:

```
inv-core/domain/state/
├─ mod.rs                   # trait StateMachine; types InvoiceState, InvoiceEvent; transition fn
├─ events.rs                # bus event shapes: .proposed, .transitioned, .entered
└─ adapter_statig.rs        # the single shipped adapter
```

The facade owns:

- The transition table, expressed as an exhaustive Rust `match` (compile-time exhaustiveness check).
- Domain enums (`InvoiceState`, `InvoiceEvent`).
- The veto seam: `propose(event)` publishes `inv.billing.invoice.proposed` and respects subscriber vetoes (matches kit's `core/stage` semantics — subscriber returns an error → transition denied).
- Post-transition emission of `.transitioned` and `.entered`.

The adapter does the translation from facade types to `statig` types and back. A second adapter (`rust-fsm`, or any other) is additive — implement `StateMachine` and gate via Cargo feature.

### 3.4 Invoice lifecycle FSM

```text
                ┌──────────────┐
                │   draft      │  mutable: lines, customer, dates
                └──────┬───────┘
                       │ issue
                       ▼
                ┌──────────────┐
        ┌───────┤   issued     │  immutable; numbered; tax frozen
        │       └──────┬───────┘
        │              │ send
        │              ▼
        │       ┌──────────────┐
        │       │    sent      │◄────── remind (sent → sent; emits reminder.sent)
        │       └──────┬───────┘
        │              │ view
        │              ▼
        │       ┌──────────────┐
        │       │   viewed     │  display state; payment can still occur
        │       └──────┬───────┘
        │              │
        │   payment    │ partial         payment full
        │   ┌──────────┼─────────────────┐
        │   ▼          ▼                 ▼
   ┌────────────┐  ┌────────────────┐  ┌────────────┐
   │  voided    │  │partially_paid  │  │   paid     │  terminal
   └────────────┘  └────────┬───────┘  └────────────┘
        ▲                    │ payment (remainder)
        │ void               └──────────────► paid
        │ allowed from issued | sent | viewed | overdue
        │ never from paid
```

- `overdue` is a derived flag and a bus event; not a state. Avoids `overdue ↔ paid` round-tripping.
- `sent` and `viewed` are transitions but not gates — payment may land any time after `issued`.
- `void` is allowed only before payment. After payment, issue a credit note.
- Credit notes are separate entities with their own FSM (`draft → issued`). Refunds (via `fin.billing.payment.refunded`) auto-create a draft credit note; the operator or agent issues it.

Recurring schedules materialise new invoices in `draft`, and auto-transition to `issued` if the schedule has `auto_issue: true` (default `false`).

### 3.5 Cross-cutting concerns

| Concern | Location |
|---|---|
| Auth / actor identity | Adapter decodes channel-native auth → `Actor`; core only sees `Actor` |
| Caller idempotency | `commands` checks `invoices.idempotency_key` before mutating state |
| Bus replay idempotency | Bus consumer checks `bus_inbox.event_id` before dispatching |
| Validation | `Input::validate()` runs first in every command |
| Authorisation (if added) | Single guard in `commands`, never per-adapter |
| FSM transition + history row + outbox row | Single DB transaction in `commands` |
| Outbound event emission | Outbox relay reads history rows where `published_at IS NULL` and publishes |
| Telemetry | `#[tracing::instrument]` on every `commands::*` function |
| Error mapping | `CoreError` → channel-native error at the adapter only |

## 4. Event bus

### 4.1 Naming convention

kit's convention (per `kit/go/runtime/bus/event.go`): topics are dot-separated as `[Source].[Category].[Object].[Action]`. Pattern matching follows MQTT: `*` matches one segment, `#` matches zero or more trailing segments. `inv` uses **`Source = inv`** and **single `Category = billing`** across all topics.

### 4.2 Emitted by `inv`

| Topic | When |
|---|---|
| `inv.billing.invoice.drafted` | New invoice created in `draft` |
| `inv.billing.invoice.issued` | `draft → issued` |
| `inv.billing.invoice.sent` | Delivered (any channel) |
| `inv.billing.invoice.viewed` | Customer opened the rendered invoice |
| `inv.billing.invoice.paid` | Marked paid |
| `inv.billing.invoice.partially_paid` | Partial payment recorded |
| `inv.billing.invoice.overdue` | Due date passed, still unpaid (flag-driven; emitted by an internal ticker) |
| `inv.billing.invoice.voided` | Voided pre-payment |
| `inv.billing.reminder.scheduled` | Reminder timer set |
| `inv.billing.reminder.sent` | Reminder dispatched |
| `inv.billing.creditnote.issued` | Credit note created |

The table above is the **domain event surface** — what consumers (`fin`, dashboards, downstream tools) subscribe to. In addition, `inv`'s FSM facade emits **mechanic events** that mirror kit's `core/stage` semantics for any tooling that wants to observe transitions generically:

| Topic | Phase | Veto-able | Source |
|---|---|---|---|
| `inv.billing.invoice.proposed` | sync (pre-transition) | yes | `commands::*` before mutation |
| `inv.billing.invoice.transitioned` | post | no | `commands::*` after commit |
| `inv.billing.invoice.entered` | post | no | `commands::*` after commit |
| `inv.billing.creditnote.proposed` | sync | yes | `commands::*` before mutation |
| `inv.billing.creditnote.transitioned` | post | no | `commands::*` after commit |
| `inv.billing.creditnote.entered` | post | no | `commands::*` after commit |

Domain events (`drafted`, `issued`, `paid`, …) describe **what happened in business terms** and are the primary integration surface. Mechanic events describe **how the FSM moved** and are mainly useful for veto subscribers, audit-replay tooling, and observability. Both sets are emitted; domain events are emitted *after* the mechanic events for the same transition.

### 4.3 Consumed by `inv`

`fin` does not currently emit bus topics (its `fin.*` strings in source today are schema-version identifiers and config keys, not bus topics). `inv` proposes the following contract; topic names are configurable so deployments can map them onto whatever `fin` (or any other publisher) ultimately ships.

| Topic | Payload (minimum) | `inv` reaction |
|---|---|---|
| `fin.billing.charge.created` | `{ charge_id, customer_id, amount, currency, lines[], due_date?, idempotency_key }` | Create draft invoice (or append line to an open draft for `customer_id`) |
| `fin.billing.payment.received` | `{ payment_id, invoice_ref?, customer_id, amount, currency, received_at, idempotency_key }` | Mark invoice paid or partially paid |
| `fin.billing.payment.refunded` | `{ refund_id, payment_id, invoice_ref, amount, currency }` | Create draft credit note against the referenced invoice |

### 4.4 Idempotency and ordering

- Every command takes an optional `idempotency_key` carried by the caller. The first request with a given key persists; subsequent requests return the original result.
- Bus events carry `event_id`. The `bus_inbox` table records every received event before processing; reprocessing the same `event_id` is a no-op.
- The outbox pattern guarantees at-least-once outbound delivery: history rows with `published_at IS NULL` are published by a relay, then marked. Subscribers must tolerate at-least-once.

## 5. Data model

All money columns are stored as `TEXT` on sqlite (canonical `rust_decimal::Decimal` string), and as `NUMERIC(20,8)` on postgres / tidb. Banker's rounding to the currency's minor-unit scale (USD/CAD: 2 decimals, DZD: 0).

```sql
customers (
  id           TEXT PRIMARY KEY,         -- external ref (ULID or caller-supplied)
  display_name TEXT NOT NULL,
  email        TEXT,
  address_json TEXT NOT NULL,            -- {country, region, city, postal, line1, line2}
  jurisdiction TEXT,                     -- ISO-3166-2, e.g. "CA-QC", "US-DE", "DZ-16"
  metadata     TEXT,
  created_at   TIMESTAMP NOT NULL,
  updated_at   TIMESTAMP NOT NULL
)

invoices (
  id              TEXT PRIMARY KEY,      -- ULID
  number          TEXT UNIQUE,           -- NULL until issued; e.g. "INV-2026-0001"
  customer_id     TEXT NOT NULL REFERENCES customers(id),
  seller_jur      TEXT NOT NULL,         -- "CA-QC" | "US-DE" | "DZ-16"
  currency        TEXT NOT NULL,         -- ISO 4217
  state           TEXT NOT NULL,         -- draft|issued|sent|viewed|partially_paid|paid|voided
  issued_at       TIMESTAMP,
  due_at          TIMESTAMP,
  sent_at         TIMESTAMP,
  viewed_at       TIMESTAMP,
  paid_at         TIMESTAMP,
  voided_at       TIMESTAMP,
  subtotal        TEXT NOT NULL,         -- Decimal
  tax_total       TEXT NOT NULL,
  total           TEXT NOT NULL,
  amount_paid     TEXT NOT NULL DEFAULT '0',
  schedule_id     TEXT,                  -- non-null if generated from a recurring schedule
  template_path   TEXT,                  -- override; falls back to config default
  pdf_blob_ref    TEXT,                  -- kit blob URI after render
  idempotency_key TEXT UNIQUE,
  nexus_review    BOOLEAN NOT NULL DEFAULT 0,  -- flagged for operator if US nexus undetermined
  metadata        TEXT,
  created_at      TIMESTAMP NOT NULL,
  updated_at      TIMESTAMP NOT NULL
)

invoice_lines (
  id           TEXT PRIMARY KEY,
  invoice_id   TEXT NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
  position     INTEGER NOT NULL,
  description  TEXT NOT NULL,
  quantity     TEXT NOT NULL,            -- Decimal
  unit_price   TEXT NOT NULL,            -- Decimal
  tax_rate_ids TEXT,                     -- JSON array of tax_rates.id applied (can be multiple)
  tax_category TEXT NOT NULL DEFAULT 'standard',  -- standard|reduced|zero_rated|exempt
  tax_amount   TEXT NOT NULL,
  line_total   TEXT NOT NULL,
  metadata     TEXT,
  UNIQUE(invoice_id, position)
)

tax_rates (                              -- mirrors tax-tables/default.toml; reloadable
  id              TEXT PRIMARY KEY,      -- e.g. "ca-qc-gst", "ca-qc-qst", "dz-tva-std"
  jurisdiction    TEXT NOT NULL,         -- seller ISO-3166-2
  applies_country TEXT,                  -- buyer country filter (NULL = any)
  applies_region  TEXT,                  -- buyer region filter (NULL = any)
  name            TEXT NOT NULL,
  rate            TEXT NOT NULL,         -- Decimal, e.g. "0.05", "0.19", "0.09975"
  category        TEXT NOT NULL,         -- standard|reduced|zero_rated|exempt
  effective_from  DATE NOT NULL,
  effective_to    DATE
)

nexus_thresholds (                       -- US economic nexus: per-state rule
  state          TEXT PRIMARY KEY,       -- e.g. "CA","NY","TX"
  revenue        TEXT,                   -- Decimal, e.g. "500000.00"
  txn_count      INTEGER,
  enabled        BOOLEAN NOT NULL DEFAULT 0
)

reminders (
  id            TEXT PRIMARY KEY,
  invoice_id    TEXT NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
  scheduled_at  TIMESTAMP NOT NULL,
  sent_at       TIMESTAMP,
  channel       TEXT NOT NULL,           -- file|stdout|bus|webhook|link
  state         TEXT NOT NULL            -- scheduled|sent|cancelled
)

schedules (
  id             TEXT PRIMARY KEY,
  customer_id    TEXT NOT NULL REFERENCES customers(id),
  template_lines TEXT NOT NULL,          -- JSON: line items to materialise each cycle
  currency       TEXT NOT NULL,
  cadence        TEXT NOT NULL,          -- "monthly@1" | "quarterly@15" | "yearly@2026-01-01"
  start_date     DATE NOT NULL,
  end_date       DATE,
  auto_issue     BOOLEAN NOT NULL DEFAULT 0,
  next_run       DATE NOT NULL,
  last_run       DATE,
  state          TEXT NOT NULL,          -- active|paused|cancelled
  metadata       TEXT
)

credit_notes (
  id             TEXT PRIMARY KEY,
  number         TEXT UNIQUE,            -- "CN-2026-0001"
  invoice_id     TEXT NOT NULL REFERENCES invoices(id),
  state          TEXT NOT NULL,          -- draft|issued
  amount         TEXT NOT NULL,          -- Decimal
  currency       TEXT NOT NULL,
  reason         TEXT,
  refund_ref     TEXT,                   -- from fin.billing.payment.refunded
  issued_at      TIMESTAMP,
  created_at     TIMESTAMP NOT NULL,
  metadata       TEXT
)

invoice_state_history (                  -- audit + transactional outbox
  id            TEXT PRIMARY KEY,        -- ULID
  invoice_id    TEXT NOT NULL REFERENCES invoices(id) ON DELETE CASCADE,
  from_state    TEXT,                    -- NULL for the initial 'drafted' row
  to_state      TEXT NOT NULL,
  event         TEXT NOT NULL,           -- transition trigger: issue|send|view|pay|partial_pay|void|...
  actor         TEXT,
  channel       TEXT NOT NULL,           -- cli|api|ws|mcp|bus
  bus_event_id  TEXT,                    -- if triggered by inbound bus event
  reason        TEXT,
  occurred_at   TIMESTAMP NOT NULL,
  published_at  TIMESTAMP,               -- outbox: NULL = pending; set by relay after bus publish
  metadata      TEXT
)
CREATE INDEX idx_inv_history_time ON invoice_state_history (invoice_id, occurred_at);
CREATE INDEX idx_inv_history_outbox ON invoice_state_history (published_at) WHERE published_at IS NULL;

credit_note_state_history (
  id              TEXT PRIMARY KEY,
  credit_note_id  TEXT NOT NULL REFERENCES credit_notes(id) ON DELETE CASCADE,
  from_state      TEXT,
  to_state        TEXT NOT NULL,
  event           TEXT NOT NULL,
  actor           TEXT,
  channel         TEXT NOT NULL,
  bus_event_id    TEXT,
  occurred_at     TIMESTAMP NOT NULL,
  published_at    TIMESTAMP,
  metadata        TEXT
)

bus_inbox (                              -- idempotent inbound event handling
  event_id      TEXT PRIMARY KEY,        -- de-dupes bus replays
  topic         TEXT NOT NULL,
  source        TEXT NOT NULL,
  received_at   TIMESTAMP NOT NULL,
  payload_json  TEXT NOT NULL,
  processed_at  TIMESTAMP,
  invoice_id    TEXT                     -- back-link if event affected an invoice
)
```

Migrations are versioned and shipped via `inv-store`. The schema is identical across sqlite / postgres / tidb modulo column-type translation (`TEXT` ↔ `NUMERIC(20,8)` for Decimal columns).

## 6. Tax engine

### 6.1 Inference algorithm

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

1. Determine buyer scope:
   - same country + same region → `domestic_local`
   - same country + different region → `domestic_other`
   - different country → `export`

2. Pick applicable rate(s):
   - `domestic_local`: look up `(seller_jur, buyer_region, line.tax_category, at)` in the rate table. Multiple rates may apply (QC → QC = GST 5 % + QST 9.975 %). Returns the set.
   - `domestic_other`:
     - **CA seller, CA buyer (non-QC):** destination-based HST/PST/GST per buyer province (table rows for QC seller → each province).
     - **US seller, US buyer (non-DE):** nexus check. If `nexus[buyer_state].enabled` and the seller has crossed `(revenue OR txn_count)` threshold for that state, apply the state's row from the table. Otherwise 0 % with `invoices.nexus_review = true`.
     - **DZ seller, DZ buyer (non-Algiers):** treat as `domestic_local` (TVA is national in DZ).
   - `export`: 0 %; line flagged `export = true` on the rendered document.

3. Apply line tax category:
   - `standard` → use the `standard` row.
   - `reduced` → use the `reduced` row if defined, else fall back to `standard` and warn.
   - `zero_rated` → 0 %, rate ID recorded.
   - `exempt` → 0 %, recorded as exempt (different audit semantics from zero-rated).

4. Compute amount in line currency at `at`. No FX. Banker's rounding to currency minor-unit scale (USD/CAD = 2, DZD = 0).

### 6.2 Tax-table file shape

`tax-tables/default.toml` ships with `inv` and is overridable via config (`tax_tables = "/path/to/custom.toml"`).

```toml
[[rate]]
id = "ca-qc-gst"
jurisdiction = "CA-QC"
applies_to_buyer = { country = "CA" }
name = "GST"
category = "standard"
rate = "0.05"
effective_from = "2008-01-01"

[[rate]]
id = "ca-qc-qst"
jurisdiction = "CA-QC"
applies_to_buyer = { country = "CA", region = "QC" }
name = "QST"
category = "standard"
rate = "0.09975"
effective_from = "2013-01-01"

# ... per-province rows for QC seller → rest-of-Canada

[[rate]]
id = "dz-tva-standard"
jurisdiction = "DZ-16"
applies_to_buyer = { country = "DZ" }
name = "TVA"
category = "standard"
rate = "0.19"
effective_from = "2017-01-01"

[[rate]]
id = "dz-tva-reduced"
jurisdiction = "DZ-16"
applies_to_buyer = { country = "DZ" }
name = "TVA reduite"
category = "reduced"
rate = "0.09"
effective_from = "2017-01-01"

[[rate]]
id = "us-ca-sales"
jurisdiction = "US-DE"
applies_to_buyer = { country = "US", region = "CA" }
name = "CA Sales Tax"
category = "standard"
rate = "0.0725"
effective_from = "2017-01-01"
# ... per US state row, all gated by nexus config

[nexus.CA]
enabled = false
revenue = "500000"
txn_count = null

[nexus.NY]
enabled = false
revenue = "500000"
txn_count = 100
```

### 6.3 Disclaimers

- `inv` is a tax calculator from *configured rates*, not a tax-law oracle. Rate accuracy and nexus thresholds are the operator's responsibility.
- Defaults are best-effort starting points sourced from public revenue-authority pages, dated, and not legal advice.
- Locality (city/county/district) sales tax is out of scope at v1. State-level US rates only. The table shape is forward-compatible (`applies_to_buyer.locality` can be added later without migration of existing rows).

## 7. Delivery channels

A `send` command takes a destination URI. v1 schemes:

| Scheme | Behaviour |
|---|---|
| `file://path/invoice.pdf` | Render PDF, write to path. |
| `stdout` | Render PDF, write to stdout. Useful for piping. |
| `bus://` | Emit `inv.billing.invoice.sent` with the rendered document inline as base64 payload. Default fallback — every successful send also emits this event regardless of scheme. |
| `webhook://example.com/hook` | POST JSON: `{ invoice, pdf_url, signature }` to the URL. `pdf_url` resolves through the same signed-link mechanism. |
| `link://` | Generate a signed URL (HMAC + expiry). Returning it is the response. The URL is served by `inv-api`'s public-view route; fetching the URL renders the invoice and emits `inv.billing.invoice.viewed`. |

The signed-link route requires `inv-api` to expose a public-readable endpoint (no auth, but URL is unguessable + expires). View tracking emits `viewed` exactly once per signed token; subsequent fetches with the same token re-render but do not re-emit.

## 8. Recurring schedules

A schedule is a recipe for materialising invoices:

```
schedule_id     ulid
customer_id     ref
template_lines  JSON array of line items (description, quantity, unit_price, tax_category)
currency        ISO 4217
cadence         "monthly@<dom>" | "quarterly@<dom>" | "yearly@<MM-DD>"
start_date      first cycle date
end_date        optional last cycle date
auto_issue      default false; if true, materialised invoice auto-transitions draft → issued
next_run        bookkeeping field, advanced by the ticker
state           active | paused | cancelled
```

An internal ticker runs daily (configurable). For every `active` schedule with `next_run <= today`:

1. Materialise a draft invoice with the schedule's template lines (current tax rates resolved at materialisation time).
2. Emit `inv.billing.invoice.drafted`. Set `invoices.schedule_id`.
3. If `auto_issue = true`, run `commands::issue_invoice` immediately and emit `inv.billing.invoice.issued`.
4. Advance `next_run` and `last_run`.

The ticker is part of `inv-server`; it can be disabled to run `inv` as a pure on-demand library if desired.

## 9. Reminders

`commands::schedule_reminder` enqueues a reminder for a given invoice with a `scheduled_at` and a `channel` (any send scheme). A ticker process scans `reminders` where `state = scheduled AND scheduled_at <= now`, performs the send, and transitions the reminder row to `state = sent`. Each transition emits `inv.billing.reminder.scheduled` / `.sent`.

Reminders never advance the invoice FSM (the invoice stays in `sent` or `viewed`). The act of reminding is a separate event class.

## 10. Channel-specific surfaces

Each adapter exposes the same operation set under its native idiom. The operation set is one-to-one with `inv-core::commands::*`.

| Operation | CLI | HTTP API | WebSocket | MCP tool |
|---|---|---|---|---|
| Draft invoice | `inv invoice draft …` | `POST /invoices` | `invoice.draft` frame | `inv_invoice_draft` |
| Issue invoice | `inv invoice issue <id>` | `POST /invoices/:id/issue` | `invoice.issue` frame | `inv_invoice_issue` |
| Send | `inv invoice send <id> --to <uri>` | `POST /invoices/:id/send` | `invoice.send` frame | `inv_invoice_send` |
| Mark paid | `inv invoice mark-paid <id>` | `POST /invoices/:id/pay` | `invoice.mark_paid` frame | `inv_invoice_mark_paid` |
| Void | `inv invoice void <id> --reason …` | `POST /invoices/:id/void` | `invoice.void` frame | `inv_invoice_void` |
| Credit | `inv creditnote draft --invoice <id>` | `POST /invoices/:id/credit-notes` | `creditnote.draft` frame | `inv_creditnote_draft` |
| Schedule (recurring) | `inv schedule create …` | `POST /schedules` | `schedule.create` frame | `inv_schedule_create` |
| Reminder | `inv reminder schedule …` | `POST /invoices/:id/reminders` | `reminder.schedule` frame | `inv_reminder_schedule` |

The HTTP API uses problem-detail responses for errors (RFC 9457). The WebSocket protocol uses JSON frames with `op`, `id`, `payload`, and a server-pushed `event` frame for bus events the client subscribes to. The MCP server exposes one tool per command plus read-only resources (`inv://invoice/{id}`, `inv://schedule/{id}`).

## 11. Configuration

`inv` reads a TOML config (kit-conventional location). All fields have defaults; the file is optional.

```toml
[storage]
backend = "sqlite"                       # sqlite | postgres | tidb
dsn = "file:./inv.sqlite"
blob = "blob://local/./inv-blobs"        # kit blob URI

[bus]
topics_in = [                            # subscriptions
  "fin.billing.charge.created",
  "fin.billing.payment.received",
  "fin.billing.payment.refunded",
]
# topic-remap supports operator-side renaming if fin uses different names:
[bus.remap]
"finance.charges.created" = "fin.billing.charge.created"

[tax]
tax_tables = "tax-tables/default.toml"   # path or list of paths (overlay order)
default_seller_jurisdiction = "CA-QC"
rounding = "bankers"

[render]
template_path = "templates/default"
pdf_engine = "wkhtmltopdf"               # or "weasyprint" — selected via Cargo feature

[link]
public_base_url = "https://invoices.example.com"
signing_key = "${INV_LINK_SIGNING_KEY}"
token_ttl = "30d"

[ticker]
enabled = true
schedules_interval = "24h"
reminders_interval = "5m"
overdue_interval = "1h"

[server]
api_listen = "127.0.0.1:7400"
ws_listen = "127.0.0.1:7401"
mcp_stdio = true
```

## 12. Open questions for the implementation plan

These are deliberately deferred to the planning step — they affect *how* but not *what*.

- PDF engine choice between `wkhtmltopdf` (mature, system binary), `weasyprint` (Python sidecar), and `printpdf` / `typst` (pure Rust). Recommendation: ship behind a Cargo feature; default to whichever works without a system dependency.
- Migration tool: `refinery` vs `sqlx::migrate!`. kit's `sqlstore` may already prefer one — check during planning.
- Authentication for `inv-api` / `inv-ws`. Out of scope at v1 surface decisions; pluggable middleware in the adapter.
- Concrete `Actor` enum shape and the auth → actor decoding for each adapter.

## 13. References

- `kit/go/runtime/bus/event.go` — canonical topic naming convention.
- `kit/go/core/stage/README.md` — pattern for transition events (`proposed`/`transitioned`/`entered`) and the veto seam.
- `kit/hops/main/templates/cli-rs/` — Rust scaffold template used to bootstrap `inv`.
- `fin/hops/main/docs/PRD.md` — `fin`'s posture (Rust, kit-parity, event-sourced, p2p ledger).
