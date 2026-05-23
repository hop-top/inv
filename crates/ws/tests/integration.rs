//! Integration tests for hop-top-inv-ws.
//!
//! Each test spins up an axum server bound to `127.0.0.1:0`, opens a
//! tokio-tungstenite client at the resulting `ws://127.0.0.1:PORT/ws`,
//! and exercises one slice of the frame protocol.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use futures_util::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use hop_top_inv_commands::{Clock, CoreCtx};
use hop_top_inv_core::domain::address::Address;
use hop_top_inv_core::domain::customer::Customer;
use hop_top_inv_core::domain::ids::CustomerId;
use hop_top_inv_core::tax::TaxTable;
use hop_top_inv_store::pool::{connect, Pool};
use hop_top_inv_store::repo::CustomerRepo;
use hop_top_inv_store::run_migrations;

use hop_top_inv_bus::Publisher;
use hop_top_inv_ws::router;
use hop_top_inv_ws::{BroadcastPublisher, SharedPublisher};

// ---------------------------------------------------------------------
// Test scaffolding
// ---------------------------------------------------------------------

struct FrozenClock(DateTime<Utc>);
impl Clock for FrozenClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
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

fn frozen_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap()
}

async fn fresh_ctx() -> (Arc<CoreCtx>, Pool) {
    let pool = connect("sqlite::memory:").await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    let (table, nexus) = TaxTable::load_from_str(FIXTURE_TOML).expect("tax fixture");
    let ctx =
        CoreCtx::new(pool.clone(), table, nexus).with_clock(Arc::new(FrozenClock(frozen_now())));
    (Arc::new(ctx), pool)
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

/// Bring up an axum server bound to an ephemeral port and return its url.
async fn start_server(ctx: Arc<CoreCtx>, publisher: SharedPublisher) -> String {
    let app = router(ctx, publisher);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    format!("ws://{}/ws", addr)
}

type WsClient =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_client(url: &str) -> WsClient {
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("ws connect");
    ws
}

async fn send_json(ws: &mut WsClient, frame: Value) {
    let text = frame.to_string();
    ws.send(WsMessage::Text(text)).await.expect("ws send");
}

async fn recv_json(ws: &mut WsClient) -> Value {
    loop {
        let msg = ws.next().await.expect("ws closed").expect("ws recv err");
        match msg {
            WsMessage::Text(t) => return serde_json::from_str(&t).expect("json"),
            WsMessage::Ping(_) | WsMessage::Pong(_) => continue,
            other => panic!("unexpected non-text frame: {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[tokio::test]
async fn connect_then_ping_round_trips() {
    let (ctx, _pool) = fresh_ctx().await;
    let publisher: SharedPublisher = Arc::new(BroadcastPublisher::default());
    let url = start_server(ctx, publisher).await;

    let mut ws = connect_client(&url).await;
    send_json(
        &mut ws,
        serde_json::json!({ "id": "1", "op": "ping", "payload": {} }),
    )
    .await;
    let resp = recv_json(&mut ws).await;
    assert_eq!(resp["id"], "1");
    assert_eq!(resp["result"]["pong"], true);
}

#[tokio::test]
async fn invoice_draft_round_trips() {
    let (ctx, pool) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let publisher: SharedPublisher = Arc::new(BroadcastPublisher::default());
    let url = start_server(ctx, publisher).await;

    let mut ws = connect_client(&url).await;
    let frame = serde_json::json!({
        "id": "draft-1",
        "op": "invoice.draft",
        "payload": {
            "customer_id": cust.to_string(),
            "seller_jurisdiction": "CA-QC",
            "currency": "CAD",
            "lines": [{
                "description": "Consulting hours",
                "quantity": "10",
                "unit_price": "125.00",
                "tax_category": "standard"
            }]
        }
    });
    send_json(&mut ws, frame).await;

    let resp = recv_json(&mut ws).await;
    assert_eq!(resp["id"], "draft-1", "got {resp}");
    let result = &resp["result"];
    assert!(result.is_object(), "expected success, got {resp}");
    assert_eq!(result["invoice"]["state"], "draft");
    assert_eq!(result["invoice"]["currency"], "CAD");
    assert_eq!(result["invoice"]["subtotal"], "1250.00");
    assert_eq!(result["lines"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn invoice_draft_missing_customer_yields_error_frame() {
    let (ctx, _pool) = fresh_ctx().await;
    let publisher: SharedPublisher = Arc::new(BroadcastPublisher::default());
    let url = start_server(ctx, publisher).await;

    let mut ws = connect_client(&url).await;
    let frame = serde_json::json!({
        "id": "err-1",
        "op": "invoice.draft",
        "payload": {
            // No customer_id at all — payload decode fails.
            "seller_jurisdiction": "CA-QC",
            "currency": "CAD",
            "lines": []
        }
    });
    send_json(&mut ws, frame).await;

    let resp = recv_json(&mut ws).await;
    assert_eq!(resp["id"], "err-1");
    assert_eq!(resp["error"]["code"], "validation", "got {resp}");
    let msg = resp["error"]["message"].as_str().expect("message string");
    assert!(
        msg.contains("customer_id") || msg.contains("missing"),
        "expected message to mention missing customer_id, got: {msg}"
    );
}

#[tokio::test]
async fn subscribe_then_publish_pushes_event_frame() {
    let (ctx, _pool) = fresh_ctx().await;
    let bus = Arc::new(BroadcastPublisher::default());
    let publisher: SharedPublisher = bus.clone();
    let url = start_server(ctx, publisher).await;

    let mut ws = connect_client(&url).await;

    // 1. Subscribe.
    send_json(
        &mut ws,
        serde_json::json!({
            "id": "sub-1",
            "op": "subscribe",
            "payload": { "pattern": "inv.billing.invoice.#" }
        }),
    )
    .await;
    let sub_resp = recv_json(&mut ws).await;
    assert_eq!(sub_resp["id"], "sub-1");
    let sub_id = sub_resp["result"]["sub_id"].as_str().unwrap().to_string();
    assert!(sub_id.starts_with("sub_"));

    // 2. Publish a matching event from outside.
    let topic = "inv.billing.invoice.drafted".to_string();
    bus.publish(
        &topic,
        serde_json::json!({"invoice_id": "invoice_test"}),
        frozen_now(),
    )
    .await
    .unwrap();

    // 3. Wait for the event frame. Cap the wait so a stuck test doesn't
    //    hang the suite indefinitely.
    let evt = tokio::time::timeout(std::time::Duration::from_secs(5), recv_json(&mut ws))
        .await
        .expect("timeout waiting for event frame");
    assert_eq!(evt["topic"], topic);
    assert_eq!(evt["payload"]["invoice_id"], "invoice_test");
    assert!(evt["timestamp"].is_string());
}

// Silence dead_code warnings — Decimal/FromStr show up via the JSON
// shaping we use elsewhere; keep the imports anchored.
#[allow(dead_code)]
fn _decimal_anchor() -> Decimal {
    Decimal::from_str("0").unwrap()
}
