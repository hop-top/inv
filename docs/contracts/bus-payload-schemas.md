# Bus payload schemas

JSON-schema sketches for every event payload `inv` emits or consumes. Source
of truth: [`crates/bus/src/events.rs`](../../crates/bus/src/events.rs),
[`crates/core/src/state/events.rs`](../../crates/core/src/state/events.rs),
[`crates/core/src/state/creditnote.rs`](../../crates/core/src/state/creditnote.rs),
[`crates/bus/src/consumer.rs`](../../crates/bus/src/consumer.rs).

## Conventions

- All payloads round-trip through `serde_json` (Serialize + Deserialize).
- **Money fields are quoted strings** (canonical `rust_decimal::Decimal`),
  not numbers. Preserves precision.
- **IDs are quoted strings** in either bare typeid form (`invoice_01J...`)
  or canonical poly-uri form (`inv://invoice/invoice_01J...`). Bus outputs
  use the URI form; inbound payloads accept either.
- **Timestamps are RFC 3339** strings (chrono `DateTime<Utc>` default
  serialisation).
- Optional fields are omitted when null (`#[serde(skip_serializing_if =
  "Option::is_none")]`).

## Emitted (domain) — invoice

### `inv.billing.invoice.drafted`

```json
{
  "invoice_id":  "invoice_01j...",
  "customer_id": "customer_01j...",
  "currency":    "CAD",
  "subtotal":    "1450.00",
  "total":       "1450.00",
  "actor":       "jad",
  "channel":     "cli"
}
```

`total == subtotal` at draft time — tax is frozen only at issue.

### `inv.billing.invoice.issued`

```json
{
  "invoice_id":   "invoice_01j...",
  "number":       "INV-2026-0001",
  "customer_id":  "customer_01j...",
  "currency":     "CAD",
  "subtotal":     "1450.00",
  "tax_total":    "217.14",
  "total":        "1667.14",
  "nexus_review": false,
  "actor":        "jad",
  "channel":      "cli"
}
```

`number` may be null if the issuer crashed mid-transaction; in practice it's
always set on the emitted event (the transaction wraps both).

### `inv.billing.invoice.sent`

```json
{
  "invoice_id": "invoice_01j...",
  "actor":      "jad",
  "channel":    "cli"
}
```

### `inv.billing.invoice.viewed`

```json
{
  "invoice_id": "invoice_01j...",
  "actor":      "public-view",
  "channel":    "api"
}
```

Always `actor = "public-view"` from the signed-link route; otherwise the
inbound actor.

### `inv.billing.invoice.paid` / `inv.billing.invoice.partially_paid`

```json
{
  "invoice_id":      "invoice_01j...",
  "amount_paid":     "1000.00",
  "cumulative_paid": "1000.00",
  "total":           "1667.14",
  "actor":           "fin",
  "channel":         "bus"
}
```

### `inv.billing.invoice.overdue`

```json
{
  "invoice_id": "invoice_01j...",
  "due_at":     "2026-04-30T00:00:00Z",
  "actor":      "inv.overdue.ticker"
}
```

Flag-only — does **not** change FSM state.

### `inv.billing.invoice.voided`

```json
{
  "invoice_id": "invoice_01j...",
  "reason":     "Customer cancelled before delivery",
  "actor":      "jad",
  "channel":    "cli"
}
```

## Emitted (domain) — credit note

### `inv.billing.creditnote.drafted`

```json
{
  "credit_note_id": "creditnote_01j...",
  "invoice_id":     "invoice_01j...",
  "amount":         "1450.00",
  "currency":       "CAD",
  "refund_ref":     "fin-refund-abc",
  "actor":          "fin",
  "channel":        "bus"
}
```

### `inv.billing.creditnote.issued`

```json
{
  "credit_note_id": "creditnote_01j...",
  "number":         "CN-2026-0001",
  "invoice_id":     "invoice_01j...",
  "amount":         "1450.00",
  "currency":       "CAD",
  "actor":          "jad",
  "channel":        "cli"
}
```

## Emitted (domain) — reminder

### `inv.billing.reminder.scheduled`

```json
{
  "reminder_id": "reminder_01j...",
  "invoice_id":  "invoice_01j...",
  "due_at":      "2026-06-15T09:00:00Z",
  "channel":     "webhook",
  "actor":       "jad"
}
```

### `inv.billing.reminder.sent`

```json
{
  "reminder_id": "reminder_01j...",
  "invoice_id":  "invoice_01j...",
  "channel":     "webhook",
  "sent_at":     "2026-06-15T09:00:01Z"
}
```

### `inv.billing.reminder.cancelled`

```json
{
  "reminder_id": "reminder_01j...",
  "invoice_id":  "invoice_01j...",
  "reason":      "operator cancelled",
  "actor":       "jad"
}
```

## Emitted (domain) — schedule

### `inv.billing.schedule.created`

```json
{
  "schedule_id": "schedule_01j...",
  "customer_id": "customer_01j...",
  "cadence":     "monthly@1",
  "currency":    "CAD",
  "auto_issue":  true,
  "start_date":  "2026-06-01",
  "actor":       "jad",
  "channel":     "cli"
}
```

### `inv.billing.schedule.paused`

```json
{
  "schedule_id": "schedule_01j...",
  "state":       "paused",
  "actor":       "jad",
  "channel":     "cli"
}
```

### `inv.billing.schedule.cancelled`

```json
{
  "schedule_id": "schedule_01j...",
  "state":       "cancelled",
  "reason":      "end_date_reached"
}
```

## Emitted (mechanic) — invoice + credit note

The mechanic-event triplet emits on every FSM transition. Topics:

- `inv.billing.invoice.proposed`
- `inv.billing.invoice.transitioned`
- `inv.billing.invoice.entered`
- `inv.billing.creditnote.proposed`
- `inv.billing.creditnote.transitioned`
- `inv.billing.creditnote.entered`

### `.proposed` (sync, veto-able)

```json
{
  "invoice_id":  "invoice_01j...",
  "from":        "Draft",
  "to":          "Issued",
  "event":       { "kind": "issue" },
  "actor":       "jad",
  "channel":     "Cli",
  "proposed_at": "2026-05-20T10:00:00Z"
}
```

Subscribers may return an error to **veto** the transition (matches kit
`core/stage` semantics). Veto reaches the commands layer as `VetoError`.

### `.transitioned` (post, informational)

```json
{
  "invoice_id":  "invoice_01j...",
  "from":        "Draft",
  "to":          "Issued",
  "event":       { "kind": "issue" },
  "actor":       "jad",
  "channel":     "Cli",
  "occurred_at": "2026-05-20T10:00:01Z"
}
```

### `.entered` (post, informational)

```json
{
  "invoice_id": "invoice_01j...",
  "state":      "Issued",
  "channel":    "Cli",
  "actor":      "jad",
  "entered_at": "2026-05-20T10:00:01Z"
}
```

For credit notes, replace `invoice_id` with `credit_note_id` and use
`CreditNoteState` values for `from` / `to` / `state`.

### Invoice event shapes inside the triplet

The `event` field carries the typed `InvoiceEvent`:

| `event.kind` | Extra fields |
|---|---|
| `"issue"` | (none) |
| `"send"` | (none) |
| `"remind"` | (none) |
| `"view"` | (none) |
| `"pay"` | `"amount_paid": "<decimal>"`, `"total": "<decimal>"` |
| `"void"` | (none) |

## Consumed topics — payload schemas

These come from publishers (typically `fin`). Names are configurable via
`[bus.remap]` so deployments can subscribe to whatever the publisher emits.

### `fin.billing.charge.created`

```json
{
  "customer_id":         "customer_01j...",
  "currency":            "CAD",
  "seller_jurisdiction": "CA-QC",
  "lines": [
    {
      "description": "Consulting",
      "quantity":    "10",
      "unit_price":  "125.00",
      "tax_category": "standard"
    }
  ],
  "due_date":        "2026-06-30T00:00:00Z",
  "idempotency_key": "fin-charge-abc"
}
```

`seller_jurisdiction` falls back to the configured default if omitted.
`idempotency_key` falls back to the inbound `event_id`.

### `fin.billing.payment.received`

```json
{
  "invoice_ref":     "inv://invoice/invoice_01j...",
  "amount":          "1450.00",
  "received_at":     "2026-05-20T10:00:00Z",
  "idempotency_key": "fin-payment-abc"
}
```

`invoice_ref` accepts either bare typeid or canonical poly-uri URI.
Required — payment without an invoice_ref is **not routable at v1**.

### `fin.billing.payment.refunded`

```json
{
  "invoice_ref": "inv://invoice/invoice_01j...",
  "amount":      "500.00",
  "reason":      "partial refund per customer request",
  "refund_id":   "fin-refund-xyz"
}
```

## Pattern subscriptions

```
inv.billing.invoice.#            every invoice topic
inv.billing.*.issued             every "issued" event (invoice + credit note)
inv.billing.reminder.sent        exact match
inv.billing.#                    everything inv emits
```

Pattern matching follows MQTT: `*` matches one segment, `#` matches zero or
more trailing segments.

## See also

- [Event-bus reference](../reference/event-bus.md) — the topic table.
- [Invoice FSM contract](invoice-state-fsm.md) — the legal transitions
  visible in `.transitioned` / `.entered`.
- Design spec [§4](../architecture/design-spec.md) — full bus design.
