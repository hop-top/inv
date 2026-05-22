//! End-to-end HTTP tests for the inv-api adapter.
//!
//! Each test boots an in-memory sqlite-backed `CoreCtx`, builds the
//! assembled router via `inv_api::router`, and drives requests through
//! `tower::ServiceExt::oneshot`. Auth is left disabled (empty
//! `bearer_tokens`) except where the test explicitly opts in.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use tower::ServiceExt;

use inv_api::{router, ApiConfig, ApiState};
use inv_commands::{Clock, CoreCtx};
use inv_core::domain::address::Address;
use inv_core::domain::customer::Customer;
use inv_core::domain::ids::CustomerId;
use inv_core::tax::TaxTable;
use inv_store::pool::{connect, Pool};
use inv_store::repo::customer::CustomerRepo;
use inv_store::run_migrations;

// =============================================================================
// fixtures
// =============================================================================

struct FrozenClock(DateTime<Utc>);

impl Clock for FrozenClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

fn frozen_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap()
}

const FIXTURE_TOML: &str = r#"
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
"#;

async fn fresh_state() -> (Arc<ApiState>, Pool) {
    fresh_state_with_config(ApiConfig {
        link_signing_key: b"test-link-key".to_vec(),
        link_ttl: Duration::from_secs(3600),
        webhook_signing_key: b"test-webhook-key".to_vec(),
        public_base_url: "http://localhost:7400".into(),
        bearer_tokens: Vec::new(),
    })
    .await
}

async fn fresh_state_with_config(config: ApiConfig) -> (Arc<ApiState>, Pool) {
    let pool = connect("sqlite::memory:").await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    let (table, nexus) = TaxTable::load_from_str(FIXTURE_TOML).expect("tax fixture");
    let ctx = CoreCtx::new(pool.clone(), table, nexus)
        .with_clock(Arc::new(FrozenClock(frozen_now())));
    let state = Arc::new(ApiState::new(Arc::new(ctx), config));
    (state, pool)
}

async fn seed_customer(pool: &Pool) -> CustomerId {
    let c = Customer {
        id: CustomerId::new(),
        display_name: "Acme Corp".into(),
        email: Some("billing@acme.example".into()),
        address: Address {
            country: "CA".into(),
            region: Some("QC".into()),
            city: Some("Montréal".into()),
            postal: None,
            line1: None,
            line2: None,
        },
        metadata: BTreeMap::new(),
        created_at: frozen_now(),
        updated_at: frozen_now(),
    };
    CustomerRepo::new(pool).save(&c).await.unwrap();
    c.id
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "non-JSON response body: {}\n{}",
            e,
            String::from_utf8_lossy(&bytes)
        )
    })
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn draft_body(customer_id: &CustomerId) -> Value {
    json!({
        "customer_id": customer_id.to_string(),
        "seller_jurisdiction": "CA-QC",
        "currency": "CAD",
        "lines": [
            {
                "description": "Consulting hours",
                "quantity": "10",
                "unit_price": "125.00",
            }
        ],
    })
}

// =============================================================================
// /healthz
// =============================================================================

#[tokio::test]
async fn healthz_returns_ok() {
    let (state, _pool) = fresh_state().await;
    let app = router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_string(resp).await, "ok");
}

// =============================================================================
// POST /v1/invoices
// =============================================================================

#[tokio::test]
async fn draft_invoice_201() {
    let (state, pool) = fresh_state().await;
    let cust = seed_customer(&pool).await;
    let app = router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/invoices")
                .header("content-type", "application/json")
                .body(Body::from(draft_body(&cust).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = body_json(resp).await;
    assert_eq!(body["invoice"]["state"], "draft");
    assert_eq!(body["invoice"]["customer_id"], cust.to_string());
    assert_eq!(body["lines"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn draft_invoice_bad_input_400() {
    let (state, pool) = fresh_state().await;
    let _cust = seed_customer(&pool).await;
    let app = router(state);

    // Empty lines -> validation error.
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/invoices")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "customer_id": "customer_doesnt_matter",
                        "seller_jurisdiction": "CA-QC",
                        "currency": "CAD",
                        "lines": [],
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp).await;
    assert_eq!(body["status"], 400);
    assert!(body["type"]
        .as_str()
        .unwrap()
        .starts_with("https://errors.inv.hop.top/"));
    assert!(body["title"].is_string());
    assert!(body["detail"].is_string());
}

#[tokio::test]
async fn issue_invoice_200() {
    let (state, pool) = fresh_state().await;
    let cust = seed_customer(&pool).await;
    let app = router(state.clone());

    // Draft first.
    let drafted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/invoices")
                .header("content-type", "application/json")
                .body(Body::from(draft_body(&cust).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let drafted_body = body_json(drafted).await;
    let inv_id = drafted_body["invoice"]["id"].as_str().unwrap().to_string();

    // Issue.
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!("/v1/invoices/{inv_id}/issue"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["invoice"]["state"], "issued");
    assert!(body["invoice"]["number"].as_str().unwrap().starts_with("INV-"));
}

#[tokio::test]
async fn pay_invoice_200() {
    let (state, pool) = fresh_state().await;
    let cust = seed_customer(&pool).await;
    let app = router(state);

    // Draft + Issue.
    let inv_id = draft_and_issue(&app, &cust).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!("/v1/invoices/{inv_id}/pay"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "amount": "9999.99" }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["fully_paid"], true);
    assert_eq!(body["invoice"]["state"], "paid");
}

#[tokio::test]
async fn unknown_id_404() {
    use inv_core::domain::ids::InvoiceId;
    let (state, _pool) = fresh_state().await;
    let app = router(state);

    // Generate a fresh, well-formed InvoiceId that doesn't exist in the
    // store. (A malformed id would short-circuit to 400 in the route's
    // parse step — that's a different code path; the 404 case
    // exercises the command's NotFound -> 404 mapping.)
    let fake_id = InvoiceId::new();
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!("/v1/invoices/{fake_id}/issue"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = body_json(resp).await;
    assert_eq!(body["status"], 404);
}

#[tokio::test]
async fn issue_twice_409() {
    let (state, pool) = fresh_state().await;
    let cust = seed_customer(&pool).await;
    let app = router(state);

    let inv_id = draft_and_issue(&app, &cust).await;

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!("/v1/invoices/{inv_id}/issue"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_json(resp).await;
    assert_eq!(body["status"], 409);
    assert!(body["type"].as_str().unwrap().contains("fsm-transition"));
}

// =============================================================================
// /v/{token}
// =============================================================================

#[tokio::test]
async fn signed_link_view_serves_html_for_valid_token() {
    let (state, pool) = fresh_state().await;
    let cust = seed_customer(&pool).await;
    let app = router(state.clone());

    let inv_id = draft_and_issue(&app, &cust).await;

    // Mint a token directly so we don't depend on /send.
    let token = inv_api::signed_link::sign(
        &inv_id,
        3600,
        &state.config.link_signing_key,
    );
    let resp = app
        .oneshot(
            Request::builder()
                .uri(&format!("/v/{token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let html = body_string(resp).await;
    assert!(
        html.to_lowercase().contains("<html") || html.contains("<!doctype"),
        "expected HTML, got: {}",
        &html[..html.len().min(200)]
    );
}

#[tokio::test]
async fn signed_link_view_expired_404() {
    let (state, pool) = fresh_state().await;
    let cust = seed_customer(&pool).await;
    let app = router(state.clone());

    let inv_id = draft_and_issue(&app, &cust).await;

    // TTL=0 → token is born expired.
    let token = inv_api::signed_link::sign(&inv_id, 0, &state.config.link_signing_key);
    let resp = app
        .oneshot(
            Request::builder()
                .uri(&format!("/v/{token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn signed_link_view_tampered_404() {
    let (state, pool) = fresh_state().await;
    let cust = seed_customer(&pool).await;
    let app = router(state.clone());

    let inv_id = draft_and_issue(&app, &cust).await;
    let token = inv_api::signed_link::sign(
        &inv_id,
        3600,
        &state.config.link_signing_key,
    );
    // Drop the last char and replace with a guaranteed-different char so
    // tampering is never a no-op (base64url tokens end in 'A' ~3% of the
    // time, which would flake CI). Invalid base64 or signature mismatch.
    let mut tampered = token;
    let last = tampered.pop();
    tampered.push(if last == Some('A') { 'B' } else { 'A' });
    let resp = app
        .oneshot(
            Request::builder()
                .uri(&format!("/v/{tampered}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// =============================================================================
// Webhook signature
// =============================================================================

#[tokio::test]
async fn webhook_send_signs_outbound_body() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let (state, pool) = fresh_state().await;
    let cust = seed_customer(&pool).await;

    // Spin up a one-shot HTTP listener that accepts the POST and
    // captures the X-Inv-Signature header + body.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();

    let captured_sig = Arc::new(tokio::sync::Mutex::new(None::<String>));
    let captured_body = Arc::new(tokio::sync::Mutex::new(Vec::<u8>::new()));
    let sig_ref = captured_sig.clone();
    let body_ref = captured_body.clone();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        // Drain the request — minimal HTTP/1.1 parsing, just enough to
        // extract one header value + body length and reply 200.
        let mut buf = vec![0u8; 16 * 1024];
        let n = sock.read(&mut buf).await.unwrap();
        let raw = &buf[..n];

        let raw_str = String::from_utf8_lossy(raw).to_string();
        let mut sig = None;
        let mut content_length: usize = 0;
        for line in raw_str.lines() {
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("x-inv-signature: ") {
                // The original (case-preserved) value sits after the colon
                // in the unaltered line; find it.
                if let Some(rest) = line.splitn(2, ':').nth(1) {
                    sig = Some(rest.trim().to_string());
                }
                let _ = v;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length: ") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
        // Strip headers, capture body.
        if let Some(idx) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let body_start = idx + 4;
            let mut body = raw[body_start..].to_vec();
            // If the body wasn't fully read, pull more bytes.
            while body.len() < content_length {
                let n = sock.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&buf[..n]);
            }
            body.truncate(content_length);
            *body_ref.lock().await = body;
        }
        *sig_ref.lock().await = sig;
        // Reply 200.
        let _ = sock
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await;
    });

    // Use a permissive HTTP client (rustls is the default for reqwest in
    // this crate; we just need a Client). State already has one.
    let app = router(state.clone());

    let inv_id = draft_and_issue(&app, &cust).await;
    let webhook_uri = format!("webhook+http://{addr}/hook");

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!("/v1/invoices/{inv_id}/send"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "destination_uri": webhook_uri }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "send via webhook failed");

    server.await.expect("listener exited cleanly");
    let sig = captured_sig.lock().await.clone().expect("X-Inv-Signature missing");
    let body = captured_body.lock().await.clone();

    // The signature must equal HMAC-SHA256(body, signing_key).
    let expected = inv_api::webhook::signature_header(&body, &state.config.webhook_signing_key);
    assert_eq!(sig, expected, "signature header mismatch");
    assert!(!body.is_empty(), "webhook receiver got empty body");
}

// =============================================================================
// helpers
// =============================================================================

async fn draft_and_issue(app: &axum::Router, cust: &CustomerId) -> String {
    let drafted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/invoices")
                .header("content-type", "application/json")
                .body(Body::from(draft_body(cust).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(drafted).await;
    let inv_id = body["invoice"]["id"].as_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!("/v1/invoices/{inv_id}/issue"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "issue failed");
    inv_id
}

// Silence unused-import lints when individual tests drop calls during
// future refactors.
#[allow(dead_code)]
fn _force_use() {
    let _: Option<Decimal> = None;
    let _ = i64::from_str("0");
}
