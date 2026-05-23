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
use inv_bus::InMemoryPublisher;
use inv_commands::{Clock, CoreCtx};
use inv_core::domain::address::Address;
use inv_core::domain::customer::Customer;
use inv_core::domain::ids::CustomerId;
use inv_core::tax::TaxTable;
use inv_store::pool::{connect, Pool};
use inv_store::repo::customer::CustomerRepo;
use inv_store::repo::history::InvoiceHistoryRepo;
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
    let (state, pool, _pub) = fresh_state_with_config_and_publisher(config).await;
    (state, pool)
}

/// Variant that wires an [`InMemoryPublisher`] into the [`CoreCtx`] so
/// tests can assert the events the api adapter fires synchronously
/// (T-0031: `inv.billing.invoice.viewed` from the `/v/{token}` route).
async fn fresh_state_with_publisher() -> (Arc<ApiState>, Pool, Arc<InMemoryPublisher>) {
    fresh_state_with_config_and_publisher(ApiConfig {
        link_signing_key: b"test-link-key".to_vec(),
        link_ttl: Duration::from_secs(3600),
        webhook_signing_key: b"test-webhook-key".to_vec(),
        public_base_url: "http://localhost:7400".into(),
        bearer_tokens: Vec::new(),
    })
    .await
}

async fn fresh_state_with_config_and_publisher(
    config: ApiConfig,
) -> (Arc<ApiState>, Pool, Arc<InMemoryPublisher>) {
    let pool = connect("sqlite::memory:").await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    let (table, nexus) = TaxTable::load_from_str(FIXTURE_TOML).expect("tax fixture");
    let publisher: Arc<InMemoryPublisher> = Arc::new(InMemoryPublisher::new());
    let ctx = CoreCtx::new(pool.clone(), table, nexus)
        .with_clock(Arc::new(FrozenClock(frozen_now())))
        .with_publisher(publisher.clone() as Arc<dyn inv_commands::Publisher>);
    let state = Arc::new(ApiState::new(Arc::new(ctx), config));
    (state, pool, publisher)
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
    assert!(body["invoice"]["number"]
        .as_str()
        .unwrap()
        .starts_with("INV-"));
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
    let token = inv_api::signed_link::sign(&inv_id, 3600, &state.config.link_signing_key);
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
async fn signed_link_view_publishes_invoice_viewed_event() {
    // T-0031: when CoreCtx carries a publisher, the /v/{token} route
    // emits `inv.billing.invoice.viewed` synchronously on the first
    // Sent → Viewed transition. Tests assert capture via InMemoryPublisher.
    let (state, pool, captured) = fresh_state_with_publisher().await;
    let cust = seed_customer(&pool).await;
    let app = router(state.clone());

    // Drive draft → issue → send (file:// to a tempdir so the FSM moves
    // into Sent; link:// would NOT advance state, and webhook:// requires
    // a live receiver).
    let inv_id = draft_and_issue(&app, &cust).await;
    let tmpdir = tempfile::tempdir().expect("tempdir");
    let dest = format!("file://{}/invoice.pdf", tmpdir.path().display());
    let send_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(&format!("/v1/invoices/{inv_id}/send"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "destination_uri": dest }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        send_resp.status(),
        StatusCode::OK,
        "send via file:// failed"
    );

    // Snapshot captured events at the boundary so we can assert the view
    // route adds exactly one new `viewed` row.
    let before = captured.captured();
    assert!(
        before
            .iter()
            .all(|e| e.topic != "inv.billing.invoice.viewed"),
        "precondition: no viewed events yet, got: {:?}",
        before.iter().map(|e| &e.topic).collect::<Vec<_>>()
    );

    // View the invoice via a freshly-minted signed token.
    let token = inv_api::signed_link::sign(&inv_id, 3600, &state.config.link_signing_key);
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

    // Assert the publisher captured exactly one new viewed event for
    // this invoice, on the canonical topic, with the right shape.
    let after = captured.captured();
    let new_viewed: Vec<_> = after
        .iter()
        .skip(before.len())
        .filter(|e| e.topic == "inv.billing.invoice.viewed")
        .collect();
    assert_eq!(
        new_viewed.len(),
        1,
        "expected exactly one viewed event, got: {:?}",
        after.iter().map(|e| &e.topic).collect::<Vec<_>>()
    );
    let payload = &new_viewed[0].payload;
    assert_eq!(payload["invoice_id"], inv_id);
    assert_eq!(payload["channel"], "api");
    assert_eq!(payload["actor"], "link.viewer");
    assert_eq!(new_viewed[0].occurred_at, frozen_now());
}

#[tokio::test]
async fn signed_link_view_tampered_404() {
    let (state, pool) = fresh_state().await;
    let cust = seed_customer(&pool).await;
    let app = router(state.clone());

    let inv_id = draft_and_issue(&app, &cust).await;
    let token = inv_api::signed_link::sign(&inv_id, 3600, &state.config.link_signing_key);
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
    use inv_core::domain::ids::InvoiceId;

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
                    json!({ "destination_uri": webhook_uri.clone() }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "send via webhook failed");
    let resp_body = body_json(resp).await;
    assert_eq!(
        resp_body["delivered_to"].as_str().unwrap(),
        webhook_uri,
        "response delivered_to should reflect the real webhook URL",
    );

    server.await.expect("listener exited cleanly");
    let sig = captured_sig
        .lock()
        .await
        .clone()
        .expect("X-Inv-Signature missing");
    let body = captured_body.lock().await.clone();

    // The signature must equal HMAC-SHA256(body, signing_key).
    let expected = inv_api::webhook::signature_header(&body, &state.config.webhook_signing_key);
    assert_eq!(sig, expected, "signature header mismatch");
    assert!(!body.is_empty(), "webhook receiver got empty body");

    // The history row must record the real webhook URL — not "stdout"
    // (which is what the v1 work-around persisted before T-0032).
    let invoice_id: InvoiceId = inv_id.parse().expect("inv id parse");
    let history = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&invoice_id)
        .await
        .expect("history list");
    let send_row = history
        .iter()
        .find(|h| h.event == "send")
        .expect("missing send history row");
    let recorded_destination = send_row
        .metadata
        .get("destination")
        .expect("send row missing destination metadata");
    assert_eq!(
        recorded_destination, &webhook_uri,
        "history row should record the real webhook URL, not stdout",
    );
}

// =============================================================================
// GET /v1/schedules
// =============================================================================

#[tokio::test]
async fn list_schedules_returns_all_seeded() {
    use chrono::NaiveDate;
    use inv_core::domain::ids::{LineId, ScheduleId};
    use inv_core::domain::invoice::TaxCategory;
    use inv_core::domain::money::Currency;
    use inv_core::domain::schedule::{Cadence, Schedule, ScheduleLine, ScheduleState};
    use inv_store::repo::schedule::ScheduleRepo;

    let (state, pool) = fresh_state().await;
    let c1 = seed_customer(&pool).await;
    let c2 = seed_customer(&pool).await;

    let sched_repo = ScheduleRepo::new(&pool);

    let mk = |cust: &CustomerId, st: ScheduleState, off: i64| Schedule {
        id: ScheduleId::new(),
        customer_id: cust.clone(),
        template_lines: vec![ScheduleLine {
            description: "Retainer".into(),
            quantity: Decimal::from_str("1").unwrap(),
            unit_price: Decimal::from_str("1000.00").unwrap(),
            tax_category: TaxCategory::Standard,
            id: LineId::new(),
        }],
        currency: Currency::CAD,
        cadence: Cadence::Monthly { dom: 1 },
        start_date: NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(),
        end_date: None,
        auto_issue: true,
        next_run: NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(),
        last_run: None,
        state: st,
        metadata: BTreeMap::new(),
        created_at: frozen_now() + chrono::Duration::seconds(off),
        updated_at: frozen_now(),
    };

    sched_repo
        .save(&mk(&c1, ScheduleState::Active, 0))
        .await
        .unwrap();
    sched_repo
        .save(&mk(&c1, ScheduleState::Paused, 1))
        .await
        .unwrap();
    sched_repo
        .save(&mk(&c2, ScheduleState::Active, 2))
        .await
        .unwrap();

    let app = router(state);

    // Cross-customer list returns all 3.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/schedules")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["schedules"].as_array().unwrap().len(), 3);

    // state=active narrows to 2.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/schedules?state=active")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["schedules"].as_array().unwrap().len(), 2);

    // customer_id narrows to c1's 2 schedules.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&format!("/v1/schedules?customer_id={c1}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["schedules"].as_array().unwrap().len(), 2);

    // limit + offset paging.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/schedules?limit=1&offset=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["schedules"].as_array().unwrap().len(), 1);

    // Invalid state -> 400.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/schedules?state=bogus")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// =============================================================================
// GET /v1/credit-notes
// =============================================================================

#[tokio::test]
async fn list_credit_notes_returns_all_seeded() {
    use inv_core::domain::creditnote::{CreditNote, CreditNoteState};
    use inv_core::domain::ids::{CreditNoteId, InvoiceId};
    use inv_core::domain::invoice::{Invoice, InvoiceState};
    use inv_core::domain::jurisdiction::Jurisdiction;
    use inv_core::domain::money::Currency;
    use inv_store::repo::credit_note::CreditNoteRepo;
    use inv_store::repo::invoice::InvoiceRepo;

    let (state, pool) = fresh_state().await;
    let cust = seed_customer(&pool).await;

    fn mk_inv(cust: &CustomerId) -> Invoice {
        Invoice {
            id: InvoiceId::new(),
            number: None,
            customer_id: cust.clone(),
            seller_jurisdiction: Jurisdiction::QuebecCa,
            currency: Currency::CAD,
            state: InvoiceState::Draft,
            issued_at: None,
            due_at: None,
            sent_at: None,
            viewed_at: None,
            paid_at: None,
            voided_at: None,
            subtotal: Decimal::from_str("100.00").unwrap(),
            tax_total: Decimal::ZERO,
            total: Decimal::from_str("100.00").unwrap(),
            amount_paid: Decimal::ZERO,
            schedule_id: None,
            template_path: None,
            pdf_blob_ref: None,
            idempotency_key: None,
            nexus_review: false,
            metadata: BTreeMap::new(),
            created_at: frozen_now(),
            updated_at: frozen_now(),
        }
    }

    let inv1 = mk_inv(&cust);
    let inv2 = mk_inv(&cust);
    InvoiceRepo::new(&pool).save(&inv1).await.unwrap();
    InvoiceRepo::new(&pool).save(&inv2).await.unwrap();

    let cn_repo = CreditNoteRepo::new(&pool);
    let mk_cn = |inv: &InvoiceId, st: CreditNoteState, off: i64| CreditNote {
        id: CreditNoteId::new(),
        number: None,
        invoice_id: inv.clone(),
        state: st,
        amount: Decimal::from_str("25.00").unwrap(),
        currency: Currency::CAD,
        reason: None,
        refund_ref: None,
        issued_at: None,
        created_at: frozen_now() + chrono::Duration::seconds(off),
        metadata: BTreeMap::new(),
    };
    cn_repo
        .save(&mk_cn(&inv1.id, CreditNoteState::Draft, 0))
        .await
        .unwrap();
    cn_repo
        .save(&mk_cn(&inv1.id, CreditNoteState::Issued, 1))
        .await
        .unwrap();
    cn_repo
        .save(&mk_cn(&inv2.id, CreditNoteState::Draft, 2))
        .await
        .unwrap();

    let app = router(state);

    // Cross-invoice list returns all 3.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/credit-notes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["credit_notes"].as_array().unwrap().len(), 3);

    // state=draft narrows to 2.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/credit-notes?state=draft")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["credit_notes"].as_array().unwrap().len(), 2);

    // invoice_id narrows to inv1's 2 notes.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&format!("/v1/credit-notes?invoice_id={}", inv1.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["credit_notes"].as_array().unwrap().len(), 2);

    // limit + offset paging.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/credit-notes?limit=1&offset=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    assert_eq!(body["credit_notes"].as_array().unwrap().len(), 1);

    // Invalid state -> 400.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/v1/credit-notes?state=bogus")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
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
