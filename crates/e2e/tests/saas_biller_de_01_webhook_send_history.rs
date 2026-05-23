//! Story saas-biller-de-01: webhook delivery records realised URL in
//! invoice_state_history.
//!
//! Surfaces: HTTP API (POST /v1/invoices/{id}/send with webhook://),
//! store (invoices + invoice_state_history with destination metadata),
//! outbound HTTP signed POST.
//!
//! xrr: NOT used. The "external boundary" (the customer's webhook
//! endpoint) is replaced by an in-process TcpListener — same pattern as
//! crates/api/tests/integration.rs::webhook_send_signs_outbound_body.
//! xrr would have to intercept hop-top-inv-api's reqwest::Client, which lives
//! inside ApiState and is not a stable seam at this version.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tower::ServiceExt;

use hop_top_inv_api::router;
use hop_top_inv_core::domain::ids::InvoiceId;
use hop_top_inv_store::repo::history::InvoiceHistoryRepo;

#[tokio::test]
async fn webhook_send_records_realised_url_and_signs_body() {
    let (state, pool, _captured, _blob) = common::fresh_api_state().await;
    let cust = common::seed_customer_qc(&pool).await;
    let app = router(state.clone());

    // Spin up a one-shot receiver capturing the body + X-Inv-Signature.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();

    let captured_sig = Arc::new(tokio::sync::Mutex::new(None::<String>));
    let captured_body = Arc::new(tokio::sync::Mutex::new(Vec::<u8>::new()));
    let sig_ref = captured_sig.clone();
    let body_ref = captured_body.clone();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 16 * 1024];
        let n = sock.read(&mut buf).await.unwrap();
        let raw = &buf[..n];
        let raw_str = String::from_utf8_lossy(raw).to_string();
        let mut sig = None;
        let mut content_length: usize = 0;
        for line in raw_str.lines() {
            if line.to_ascii_lowercase().starts_with("x-inv-signature: ") {
                if let Some(rest) = line.split_once(':').map(|x| x.1) {
                    sig = Some(rest.trim().to_string());
                }
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length: ") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
        if let Some(idx) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let mut body = raw[idx + 4..].to_vec();
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
        let _ = sock
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
            .await;
    });

    // Draft + issue + send via HTTP API.
    let inv_id = draft_and_issue(&app, &cust.to_string()).await;
    let webhook_uri = format!("webhook+http://{addr}/billing/inv-hook");
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/invoices/{inv_id}/send"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "destination_uri": webhook_uri.clone() }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp_body = body_json(resp).await;
    assert_eq!(resp_body["delivered_to"].as_str().unwrap(), webhook_uri);

    // Wait for the receiver to finish capturing.
    server.await.expect("receiver exited");

    // Validate signature.
    let sig = captured_sig
        .lock()
        .await
        .clone()
        .expect("X-Inv-Signature missing");
    let body = captured_body.lock().await.clone();
    let expected =
        hop_top_inv_api::webhook::signature_header(&body, &state.config.webhook_signing_key);
    assert_eq!(sig, expected, "signature header mismatch");
    assert!(!body.is_empty());

    // History row records the realised destination URI.
    let invoice_id: InvoiceId = inv_id.parse().expect("inv id parse");
    let history = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&invoice_id)
        .await
        .expect("history list");
    let send_row = history
        .iter()
        .find(|h| h.event == "send")
        .expect("send history row");
    assert_eq!(
        send_row.metadata.get("destination").map(|s| s.as_str()),
        Some(webhook_uri.as_str()),
        "history row must record the real webhook URL",
    );
    // Also: channel = api.
    use hop_top_inv_core::domain::invoice::HistoryChannel;
    assert_eq!(send_row.channel, HistoryChannel::Api);

    // Quiet timeout helper.
    tokio::time::sleep(Duration::from_millis(0)).await;
}

// =============================================================================
// helpers
// =============================================================================

async fn draft_and_issue(app: &axum::Router, customer_id: &str) -> String {
    let drafted = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/invoices")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "customer_id": customer_id,
                        "seller_jurisdiction": "CA-QC",
                        "currency": "CAD",
                        "lines": [
                            { "description": "Consulting", "quantity": "10", "unit_price": "125.00" }
                        ]
                    })
                    .to_string(),
                ))
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
                .uri(format!("/v1/invoices/{inv_id}/issue"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "issue failed");
    inv_id
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("json body")
}
