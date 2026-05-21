//! Shared application state handed to every route.
//!
//! The [`ApiState`] owns:
//!
//! - The [`inv_commands::CoreCtx`] that every command consumes.
//! - Adapter-specific config ([`ApiConfig`]): bearer-token allowlist,
//!   signed-link signing key + TTL + public base URL, and the webhook
//!   signing key for outbound deliveries.
//! - The HTTP client used for outbound webhook posts. Held here so tests
//!   can swap in a stubbed client and so we don't allocate a new
//!   `reqwest::Client` on every request.
//!
//! Cloning [`ApiState`] is cheap; we wrap in `Arc<ApiState>` at the
//! application layer because axum's `with_state` clones on every request.

use std::sync::Arc;
use std::time::Duration;

use inv_commands::CoreCtx;

/// Config knobs the api adapter needs but the command layer doesn't.
#[derive(Debug, Clone)]
pub struct ApiConfig {
    /// Allowed bearer tokens. Requests to `/v1/*` must present
    /// `Authorization: Bearer <token>` matching one entry. Empty list
    /// disables authentication (useful for local dev — never for prod).
    pub bearer_tokens: Vec<String>,

    /// HMAC key used to sign + verify the public `/v/{token}` view links.
    pub link_signing_key: Vec<u8>,
    /// Default TTL for newly-minted signed links.
    pub link_ttl: Duration,
    /// Public origin (scheme + host) used to render absolute signed-link
    /// URLs back to the caller (e.g. `https://invoices.example.com`).
    pub public_base_url: String,

    /// HMAC key used to sign outbound webhook bodies. Receivers verify by
    /// computing the same HMAC-SHA256 over the raw bytes and comparing
    /// against the `X-Inv-Signature` header.
    pub webhook_signing_key: Vec<u8>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            bearer_tokens: Vec::new(),
            link_signing_key: Vec::new(),
            link_ttl: Duration::from_secs(60 * 60 * 24 * 30), // 30 days
            public_base_url: "http://localhost:7400".into(),
            webhook_signing_key: Vec::new(),
        }
    }
}

/// Per-request application state.
#[derive(Clone)]
pub struct ApiState {
    /// Command dependencies.
    pub ctx: Arc<CoreCtx>,
    /// Adapter-specific knobs.
    pub config: ApiConfig,
    /// HTTP client used for outbound webhook posts.
    pub http: reqwest::Client,
}

impl ApiState {
    /// Construct with the default [`reqwest::Client`] (rustls).
    pub fn new(ctx: Arc<CoreCtx>, config: ApiConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { ctx, config, http }
    }

    /// Override the HTTP client (used by tests so outbound webhook
    /// dispatch can point at a local mock listener).
    pub fn with_http(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiState")
            .field("ctx", &self.ctx)
            .field(
                "config.bearer_tokens",
                &format_args!("[{} entries]", self.config.bearer_tokens.len()),
            )
            .field("config.link_ttl", &self.config.link_ttl)
            .field("config.public_base_url", &self.config.public_base_url)
            .finish()
    }
}
