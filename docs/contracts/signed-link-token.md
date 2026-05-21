# Signed-link token format

Wire shape for tokens served by `inv server`'s public `GET /v/{token}`
route — the `link://` delivery channel. Authored from
[`crates/api/src/signed_link.rs`](../../crates/api/src/signed_link.rs).

## Goal

Mint an unguessable, time-bounded URL that anyone can fetch (no auth) to
render an invoice. The URL is the **delivery medium**, so its security
relies on:

- An unpredictable HMAC signature.
- A short expiry (configurable via `[link].token_ttl`, default `30d`).

## Token shape

```
token = base64url_no_pad( invoice_id ':' expiry_unix ':' sig )

sig   = lowercase_hex( HMAC-SHA256( signing_key, invoice_id ':' expiry_unix ) )
```

Where:

- `invoice_id` is the typeid form (e.g. `invoice_01j...`).
- `expiry_unix` is Unix-seconds (decimal integer).
- `signing_key` is the `[link].signing_key` value from `inv.toml` (typically
  read from env via `${INV_LINK_SIGNING_KEY}`).
- `:` is the inner separator. Typeids never contain `:`, so it's
  unambiguous.
- The outer `base64url_no_pad` makes the token URL-safe out of the box.

The HMAC digest is rendered in **lowercase hex** to keep the inner payload
printable; the outer base64url wrap is what gives the compact URL form.

## Wire example

```
invoice_id = "invoice_01j9zlabcdef"
expiry     = 1748793600           # 2026-06-01T00:00:00Z
key        = b"hunter2-don't-do-this-in-prod"

inner   = "invoice_01j9zlabcdef:1748793600"
sig     = HMAC-SHA256(key, inner) → "d2e5...abcd"  (lowercase hex)
composed = "invoice_01j9zlabcdef:1748793600:d2e5...abcd"
token   = base64url_no_pad(composed)
```

Mint: `sign(invoice_id, ttl_secs, signing_key) -> token`.

Verify: `verify(token, signing_key) -> Result<VerifiedToken, VerifyError>`.

## Verification

`verify_at(token, signing_key, now_unix)` returns the embedded
`VerifiedToken { invoice_id, expiry_unix }` on success. Three failure
modes, all collapsed to a single 404 by the public route (to prevent
attackers from probing valid invoice IDs):

| `VerifyError` | Trigger |
|---|---|
| `Base64` | Outer base64url didn't decode. |
| `Payload` | Inner string isn't `id:exp:sig` shaped. |
| `Expired` | `expiry_unix <= now_unix`. |
| `Mismatch` | HMAC signature didn't match. |

The public route maps all of these to `ApiError::InvalidLink` → HTTP 404 with
problem-detail body. Clients see no signal about which check failed.

## Constant-time comparison

The signature comparison uses a constant-time byte-equality check
(`constant_time_eq` in [signed_link.rs](../../crates/api/src/signed_link.rs))
to avoid timing-side-channel leaks of the expected signature.

## Side effects on successful fetch

A successful `GET /v/{token}`:

1. Verifies the token.
2. Resolves the invoice from `InvoiceRepo`.
3. Renders the invoice HTML (the route returns HTML, not PDF, so it renders
   in-browser).
4. Emits `inv.billing.invoice.viewed` — **exactly once per token**.

Subsequent fetches with the same token re-render but **do not** re-emit
`.viewed`. The de-dup uses the token's signature as the cache key (so two
distinct tokens for the same invoice both emit `.viewed`, once each).

## Choosing a signing key

- Use a high-entropy random key (≥ 32 bytes).
- Store it via env (`${INV_LINK_SIGNING_KEY}`), not in-repo.
- Rotate the key by minting new tokens with the new key — old tokens (signed
  with the old key) will fail verification and become invalid links. There
  is no key-versioning at v1 (single static key).

## Choosing a TTL

- Default: `30d`. Suitable for "view your invoice for a month" workflows.
- Short TTL (hours) for one-shot share links sent via Slack / email.
- Long TTL (90d+) for archival "always link to the invoice from your
  customer portal".

The tradeoff is leak window: a leaked URL grants access until expiry.

## Limitations at v1

- **One signing key.** Rotation invalidates all existing tokens at once.
- **No revocation.** Once minted, a token is valid until its embedded
  expiry. To revoke a specific link, rotate the signing key (which revokes
  everything).
- **No CSRF / replay protection** beyond the HMAC + expiry. Anyone with the
  URL can fetch.

## Source

- [`crates/api/src/signed_link.rs`](../../crates/api/src/signed_link.rs) —
  sign / verify functions + tests.
- [`crates/api/src/handlers/view.rs`](../../crates/api/src/handlers/view.rs) —
  the public route + the once-per-token `.viewed` emission.
- [`config.md`](../reference/config.md#link) — `[link]` config block.
