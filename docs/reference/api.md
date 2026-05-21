# HTTP API reference

Every route exposed by `inv-api`. Authored from
[`crates/api/src/lib.rs`](../../crates/api/src/lib.rs) and
[`crates/api/src/handlers/`](../../crates/api/src/handlers/).

The server runs inside `inv server` (default `127.0.0.1:7400`). Routes are
divided into three groups:

- **Public** (no auth): `GET /healthz`, `GET /v/{token}`.
- **Authenticated `/v1/*`** (bearer-token, `Authorization: Bearer <token>`):
  every operational endpoint.
- **Operational `/tick/*`**: same auth as `/v1/*`; driven by `inv server`'s
  internal ticker on a timer.

Errors use RFC 9457 `application/problem+json`.

## Public routes

| Method | Path | Description |
|---|---|---|
| GET | `/healthz` | Liveness probe. Always 200. |
| GET | `/v/{token}` | Public signed-link view. Renders the invoice's HTML/PDF and emits `inv.billing.invoice.viewed` (exactly once per token). See [contracts/signed-link-token.md](../contracts/signed-link-token.md). |

## Authenticated `/v1/*` routes

### Invoices

| Method | Path | Action |
|---|---|---|
| POST | `/v1/invoices` | Draft. |
| GET | `/v1/invoices` | List (with filters). |
| GET | `/v1/invoices/{id}` | Show. |
| POST | `/v1/invoices/{id}/issue` | Issue. |
| POST | `/v1/invoices/{id}/send` | Send to URI. |
| POST | `/v1/invoices/{id}/pay` | Record payment. |
| POST | `/v1/invoices/{id}/void` | Void (pre-payment only). |

### Credit notes

| Method | Path | Action |
|---|---|---|
| POST | `/v1/invoices/{id}/credit-notes` | Draft credit note against invoice. |
| GET | `/v1/credit-notes` | List. |
| GET | `/v1/credit-notes/{id}` | Show. |
| POST | `/v1/credit-notes/{id}/issue` | Issue (terminal). |

### Schedules

| Method | Path | Action |
|---|---|---|
| POST | `/v1/schedules` | Create. |
| GET | `/v1/schedules` | List (`?customer_id=` filter). |
| GET | `/v1/schedules/{id}` | Show. |
| POST | `/v1/schedules/{id}/pause` | Pause. |
| POST | `/v1/schedules/{id}/cancel` | Cancel (terminal). |

### Reminders

| Method | Path | Action |
|---|---|---|
| POST | `/v1/invoices/{id}/reminders` | Schedule a reminder. |
| GET | `/v1/reminders` | List. |
| POST | `/v1/reminders/{id}/cancel` | Cancel. |

### Customers

| Method | Path | Action |
|---|---|---|
| POST | `/v1/customers` | Create. |
| GET | `/v1/customers` | List. |
| GET | `/v1/customers/{id}` | Show. |

### Tickers (operator-triggered)

| Method | Path | Action |
|---|---|---|
| POST | `/v1/tick/schedules` | Run the schedules ticker once. |
| POST | `/v1/tick/reminders` | Run the reminders ticker once. |
| POST | `/v1/tick/overdue` | Run the overdue-marker ticker once. |

### Tax

| Method | Path | Action |
|---|---|---|
| GET | `/v1/tax/rates` | Dump the loaded tax table. |

## Request / response shapes

Bodies are JSON. Field names use snake_case. Money fields are quoted strings
(canonical `rust_decimal::Decimal` string), not numbers — preserves precision.

### Draft request body (POST `/v1/invoices`)

```json
{
  "customer_id": "customer_01...",
  "currency": "CAD",
  "seller_jurisdiction": "CA-QC",
  "lines": [
    {
      "description": "Consulting",
      "quantity": "10",
      "unit_price": "125.00",
      "tax_category": "standard"
    }
  ],
  "due_at": "2026-06-30T00:00:00Z",
  "idempotency_key": "draft-2026-06-acme-1",
  "template_path": null
}
```

### Send request body (POST `/v1/invoices/{id}/send`)

```json
{
  "destination_uri": "file:///tmp/inv.pdf",
  "idempotency_key": null
}
```

`destination_uri` accepts the same schemes as the CLI's `--to` flag:
`file://`, `stdout`, `bus://`, `webhook://`, `link://`.

### Pay request body (POST `/v1/invoices/{id}/pay`)

```json
{
  "amount": "1450.00",
  "received_at": "2026-05-20T10:00:00Z",
  "idempotency_key": null,
  "bus_event_id": null
}
```

For exhaustive wire shapes, the matching WS adapter mirrors API field names
1:1 — see [ws.md](ws.md).

## Error responses (RFC 9457)

```json
{
  "type":   "https://errors.inv.hop.top/<slug>",
  "title":  "Short human title",
  "status": 400,
  "detail": "Long-form detail",
  "instance": "/v1/invoices/invoice_01..."
}
```

Content-Type: `application/problem+json`. Mapping table (from
[`crates/api/src/error.rs`](../../crates/api/src/error.rs)):

| Slug | HTTP | Trigger |
|---|---|---|
| `validation` | 400 | `CoreError::Validation` — bad input. |
| `idempotency-conflict` | 409 | `CoreError::Idempotency` — replay with different shape. |
| `fsm-transition` | 409 | `CoreError::FsmTransition` — illegal transition. |
| `not-found` | 404 | Entity missing. |
| `not-implemented` | 501 | Feature gated off. |
| `repo` | 500 | DB / repo failure. |
| `tax` | 500 | Tax engine failure. |
| `render` | 500 | Template / HTML render failure. |
| `pdf` | 500 | PDF render failure. |
| `bad-request` | 400 | Body parse failure. |
| `unauthorized` | 401 | Missing / bad bearer. |
| `invalid-link` | 404 | Signed-link verification failed (collapsed; see [contracts/signed-link-token.md](../contracts/signed-link-token.md)). |
| `webhook-dispatch-failed` | 502 | Outbound webhook target rejected the POST. |
| `internal` | 500 | Catch-all. |

## Auth

Bearer-token middleware ([`crates/api/src/auth.rs`](../../crates/api/src/auth.rs)).
The token list is config-driven — at v1 a single static list of accepted
tokens; real auth (JWT, OAuth, mTLS) is pluggable middleware deferred per
design §12. The public `/v/{token}` and `/healthz` routes are exempt.

## Webhook signature (outbound)

When `invoice.send` resolves to `webhook://...`, the POSTed body carries an
`X-Inv-Signature: sha256=<hex>` header. The signature is HMAC-SHA256 over the
raw request body, keyed by `webhook_signing_key` from config. Receivers
should validate before trusting the payload.
