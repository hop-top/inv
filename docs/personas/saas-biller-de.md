# SaaS biller (Berlin, fin+inv as a service)

> Small-team SaaS company in DE. Runs `inv server` behind their product app
> as a billing webhook integration. Payment events come in through `fin` from
> Stripe webhooks. Customers in DE, rest-of-EU, and US.

## Context

- Seller jurisdiction `US-DE` (Delaware-incorporated holdco; team is in
  Berlin). Note: at v1 the only US seller modelled is `US-DE`. Treat the
  legal-entity jurisdiction as authoritative; the operating location is
  irrelevant to the tax engine.
- Product app calls `inv` over HTTP (`/v1/invoices` + friends) with a static
  bearer token. `inv server` runs as a long-lived process; outbox relay,
  schedule ticker, reminders ticker all on.
- `fin` is also running, fed by Stripe webhooks. `fin.billing.charge.created`
  and `fin.billing.payment.received` flow into `inv` over the shared bus.
- No agent. No MCP. Bus + HTTP only.
- Outbound notifications use `webhook://` to call the product app back when
  invoices transition — the app decides what to render in-product.

## What they want from inv

- HTTP `POST /v1/invoices/{id}/send` with `destination_uri =
  "webhook://..."` and deterministic delivery: each send records the
  realised URL so they can replay/audit. See
  [reference/api.md](../reference/api.md#authenticated-v1-routes) and
  [user/how-to-issue.md](../user/how-to-issue.md).
- Idempotent send: re-sending the same invoice MUST NOT double-publish to
  the bus. The `idempotency_key` on every command + `bus_inbox` dedup are
  the load-bearing guards. See
  [reference/event-bus.md#idempotency--ordering-design-44](../reference/event-bus.md#idempotency--ordering-design-44).
- Bearer-token auth on every `/v1/*` route, with timing-attack resistance
  on the comparison. See [reference/api.md#auth](../reference/api.md#auth).
- Webhook payload signature: `X-Inv-Signature: sha256=<hex>` so the product
  app can verify before trusting. See
  [reference/api.md#webhook-signature-outbound](../reference/api.md#webhook-signature-outbound).
- `fin → inv` payment auto-marking via `fin.billing.payment.received`. The
  product app never has to call `POST /pay` itself. See
  [reference/event-bus.md](../reference/event-bus.md#consumed-topics).
- Refund-driven credit notes via `fin.billing.payment.refunded`. Operator
  reviews + issues. See [contracts/creditnote-state-fsm.md](../contracts/creditnote-state-fsm.md).

## What they don't need (or want hidden)

- CLI for steady-state. CLI is for ops chores (drain outbox, inspect
  schedule, kick a ticker), not the hot path.
- MCP. No agent surface needed at v1.
- Signed-link `link://` delivery — they have their own customer portal that
  the product app renders.
- Tax tables for DZ. Strip them out of the loaded table if file size matters,
  or leave them — they don't match a DE-seller invoice anyway.

## Known gaps for this persona at v1

- **No reverse-charge VAT for EU B2B.** This is a hard gap. The shipped
  seller jurisdictions are `CA-QC | US-DE | DZ-16`; there is no `DE` seller
  row in `tax-tables/default.toml`, no EU intra-community supply handling,
  no VAT-ID validation, no reverse-charge marker on the rendered document.
  A DE-incorporated SaaS billing EU B2B customers either has to (a) model
  the entity as `US-DE` and accept that EU-VAT compliance happens outside
  `inv`, or (b) wait for the EU seller jurisdictions to ship. There is no
  in-product workaround at v1. See design
  [§2.2 (out of scope)](../architecture/design-spec.md#22-out-of-scope-v1)
  and [contracts/tax-resolution.md](../contracts/tax-resolution.md).
- **Webhook delivery is fire-and-forget at the FSM level.** A `webhook://`
  send that the receiver 5xx's surfaces as
  `webhook-dispatch-failed` (HTTP 502 from the `/send` route) but does not
  auto-retry. The product app needs its own retry loop or a poll on
  `GET /v1/invoices/{id}` to confirm the `sent` transition landed.
- **No JWT / OAuth auth at v1.** Bearer is a static list from config. JWT,
  OAuth, mTLS are deferred (design §12). Rotate by editing config + restart.
- **Hot-reload of tax tables not supported.** Restart `inv server` to pick
  up new rates. See [user/how-to-tax-config.md#reload-after-edits](../user/how-to-tax-config.md#reload-after-edits).

## Reading path

1. [architecture/design-spec.md](../architecture/design-spec.md) §1–§4 — composition: one core, thin adapters, bus topology.
2. [user/how-to-issue.md](../user/how-to-issue.md) HTTP API section — draft → issue → send via curl.
3. [reference/api.md](../reference/api.md) — full route table + error mapping + webhook signature.
4. [reference/event-bus.md](../reference/event-bus.md) — `fin.billing.*` consumption + `inv.billing.*` emission.
5. [contracts/bus-payload-schemas.md](../contracts/bus-payload-schemas.md) — payload shapes the product app subscribes to.
6. [contracts/invoice-state-fsm.md](../contracts/invoice-state-fsm.md) — what re-sends and payments do to state.
7. [contracts/tax-resolution.md](../contracts/tax-resolution.md) — confirm the v1 limits before designing around them.
8. [user/troubleshooting.md](../user/troubleshooting.md) — outbox + `nexus_review` + webhook failures.
