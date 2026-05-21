# WebSocket reference

Frame protocol + every op. Authored from
[`crates/ws/src/frames.rs`](../../crates/ws/src/frames.rs) and
[`crates/ws/src/ops.rs`](../../crates/ws/src/ops.rs).

The WebSocket endpoint upgrades from `/ws` on the same listener as the HTTP
API (`inv server --listen 127.0.0.1:7400` → `ws://127.0.0.1:7400/ws`).

## Frame shapes (JSON)

### Request (client → server)

```jsonc
{ "id": "1", "op": "invoice.draft", "payload": { ... } }
```

### Subscribe / unsubscribe

```jsonc
{ "id": "2", "op": "subscribe",   "payload": { "pattern": "inv.billing.invoice.#" } }
{ "id": "3", "op": "unsubscribe", "payload": { "sub_id": "sub_1" } }
```

`pattern` follows MQTT-style segments: `*` matches one dot-segment, `#`
matches zero or more trailing segments.

### Response (server → client)

```jsonc
{ "id": "1", "result": { ... } }
{ "id": "1", "error":  { "code": "validation", "message": "..." } }
```

### Server-pushed event (server → client; for matching subscriptions)

```jsonc
{ "topic": "inv.billing.invoice.issued", "payload": { ... }, "timestamp": "2026-..." }
```

## Op surface

Sixteen ops + the two subscription-management ops + `ping`. From the
`OP_NAMES` constant in
[`crates/ws/src/ops.rs`](../../crates/ws/src/ops.rs):

### Invoice

| Op | Payload |
|---|---|
| `invoice.draft` | `{ customer_id, seller_jurisdiction, currency, lines[], idempotency_key?, due_at?, template_path? }` |
| `invoice.issue` | `{ invoice_id, idempotency_key? }` |
| `invoice.send` | `{ invoice_id, destination_uri, idempotency_key? }` |
| `invoice.pay` | `{ invoice_id, amount, received_at?, idempotency_key?, bus_event_id? }` |
| `invoice.void` | `{ invoice_id, reason?, idempotency_key? }` |

### Credit note

| Op | Payload |
|---|---|
| `creditnote.draft` | `{ invoice_id, amount, reason?, refund_ref?, idempotency_key? }` |
| `creditnote.issue` | `{ credit_note_id, idempotency_key? }` |

### Schedule

| Op | Payload |
|---|---|
| `schedule.create` | `{ customer_id, template_lines[], currency, cadence, start_date, end_date?, auto_issue? }` |
| `schedule.pause` | `{ schedule_id }` |
| `schedule.cancel` | `{ schedule_id }` |

### Reminder

| Op | Payload |
|---|---|
| `reminder.schedule` | `{ invoice_id, scheduled_at, channel_scheme }` |
| `reminder.cancel` | `{ reminder_id }` |

### Tickers

| Op | Payload |
|---|---|
| `tick.schedules` | `{}` |
| `tick.reminders` | `{}` |
| `tick.overdue` | `{}` |

### Liveness

| Op | Payload |
|---|---|
| `ping` | `{}` → `{ "pong": true }` |

### Subscription management

| Op | Payload | Result |
|---|---|---|
| `subscribe` | `{ pattern }` | `{ sub_id }` — passed back to `unsubscribe` |
| `unsubscribe` | `{ sub_id }` | `{ unsubscribed: true }` |

## Error envelope

```jsonc
{
  "id": "1",
  "error": {
    "code": "validation",
    "message": "amount: must be positive"
  }
}
```

Error codes (from `OpError::from_core` in
[`crates/ws/src/ops.rs`](../../crates/ws/src/ops.rs)):

| Code | Meaning |
|---|---|
| `validation` | Input failed validation. |
| `idempotency` | Idempotency-key conflict. |
| `not_found` | Entity not found. |
| `not_implemented` | Feature gated off. |
| `fsm_transition` | Illegal FSM transition. |
| `tax` | Tax engine error. |
| `render` | HTML render or PDF render error. |
| `repo` | DB / repo error. |
| `unknown_op` | Op name not recognised. |
| `internal` | Catch-all. |

## Topic-pattern semantics

The bus uses kit's dot-segmented naming: `[Source].[Category].[Object].[Action]`.
`inv` always emits with `Source = inv` and `Category = billing`. Useful
subscribe patterns:

- `inv.billing.invoice.#` — every invoice topic (domain + mechanic).
- `inv.billing.*.issued` — every "issued" event (invoice + credit note).
- `inv.billing.reminder.sent` — exact match.
- `inv.billing.#` — everything `inv` emits.

A WebSocket client can subscribe to multiple patterns concurrently. The
server pushes one `EventFrame` per match; if an event matches N of your
subscriptions, you receive it N times.

## See also

- [Bus reference](event-bus.md) — every topic + payload schema.
- [Contracts → bus payload schemas](../contracts/bus-payload-schemas.md).
