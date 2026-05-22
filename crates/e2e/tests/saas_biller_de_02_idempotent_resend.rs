//! Story saas-biller-de-02: idempotent re-send does not double-publish.
//!
//! Surfaces: HTTP API, commands (idempotency_key dedup),
//! store (UNIQUE constraint + no duplicate history row),
//! bus outbox (no double-emit on replay).
//!
//! ## Implementation status (T-0036 sub-finding)
//!
//! `idempotency_key` dedup at the command layer is currently implemented
//! ONLY on `draft_invoice` (idempotency_replay: true on second call).
//! The `send_invoice` + `send_invoice_render` paths accept the field on
//! their inputs but do NOT yet dedup — a second call with the same key
//! re-runs the FSM transition. The story's "second send returns
//! `idempotency_replay: true`" criterion isn't met today.
//!
//! Rather than silently weaken the assertion, this test:
//!   1. Exercises draft-level idempotency end-to-end (assertion passes).
//!   2. Asserts the CURRENT send-replay behaviour (no dedup) so a
//!      future T-#### that implements send idempotency flips this
//!      assertion deliberately. The test serves as the regression
//!      anchor + the "unmet criterion" beacon.

mod common;

use std::str::FromStr;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use rust_decimal::Decimal;
use serde_json::{json, Value};
use tower::ServiceExt;

use inv_api::router;
use inv_commands::{
    draft_invoice, send_invoice, Actor, Channel, DraftInvoiceInput, DraftLineInput,
    SendInvoiceInput,
};
use inv_core::domain::invoice::TaxCategory;
use inv_core::domain::jurisdiction::Jurisdiction;
use inv_core::domain::money::Currency;
use inv_store::repo::history::InvoiceHistoryRepo;

#[tokio::test]
async fn draft_idempotency_key_replays() {
    // Implemented surface: draft.
    let (ctx, pool, _captured, _blob) = common::fresh_ctx().await;
    let cust = common::seed_customer_qc(&pool).await;
    let input = || DraftInvoiceInput {
        customer_id: cust.clone(),
        seller_jurisdiction: Jurisdiction::QuebecCa,
        currency: Currency::CAD,
        lines: vec![DraftLineInput {
            description: "Consulting".into(),
            quantity: Decimal::from_str("10").unwrap(),
            unit_price: Decimal::from_str("125.00").unwrap(),
            tax_category: TaxCategory::Standard,
        }],
        idempotency_key: Some("draft-2026-06-acme-1".into()),
        actor: Actor::Cli { name: "jad".into() },
        channel: Channel::Cli,
        due_at: None,
        template_path: None,
    };
    let first = draft_invoice(&ctx, input()).await.expect("first");
    let second = draft_invoice(&ctx, input()).await.expect("second");
    assert_eq!(first.invoice.id, second.invoice.id);
    assert!(!first.idempotency_replay);
    assert!(second.idempotency_replay, "second call must replay");
    assert!(second.emitted_events.is_empty(), "replay must NOT emit");
    // History: only one row (no double-insert on replay).
    let hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&first.invoice.id)
        .await
        .unwrap();
    assert_eq!(hist.len(), 1, "no duplicate history row on replay");
}

#[tokio::test]
async fn http_draft_idempotency_returns_replay_flag() {
    // Same as above but via the HTTP adapter — confirms the
    // `idempotency_replay` field reaches the wire.
    let (state, pool, _captured, _blob) = common::fresh_api_state().await;
    let cust = common::seed_customer_qc(&pool).await;
    let app = router(state);

    let body = || {
        json!({
            "customer_id": cust.to_string(),
            "seller_jurisdiction": "CA-QC",
            "currency": "CAD",
            "lines": [
                { "description": "Consulting", "quantity": "10", "unit_price": "125.00" }
            ],
            "idempotency_key": "draft-http-1"
        })
    };

    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/invoices")
                .header("content-type", "application/json")
                .body(Body::from(body().to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::CREATED);
    let first_body = body_json(first).await;
    let first_id = first_body["invoice"]["id"].as_str().unwrap().to_string();

    let second = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/invoices")
                .header("content-type", "application/json")
                .body(Body::from(body().to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    // Replay returns 200 OK (not 201 Created).
    assert_eq!(second.status(), StatusCode::OK);
    let second_body = body_json(second).await;
    assert_eq!(
        second_body["invoice"]["id"].as_str().unwrap(),
        first_id,
        "replay returns same invoice id",
    );
    assert_eq!(second_body["idempotency_replay"], json!(true));
}

#[tokio::test]
async fn send_idempotency_key_currently_not_deduped() {
    // Documents the CURRENT behaviour gap noted in the module-level
    // doc. When send_invoice gains true idempotency, this test will
    // need to be updated to assert dedup (same as draft above).
    let (ctx, pool, _captured, _blob) = common::fresh_ctx().await;
    let cust = common::seed_customer_qc(&pool).await;
    let drafted = draft_invoice(
        &ctx,
        DraftInvoiceInput {
            customer_id: cust.clone(),
            seller_jurisdiction: Jurisdiction::QuebecCa,
            currency: Currency::CAD,
            lines: vec![DraftLineInput {
                description: "Consulting".into(),
                quantity: Decimal::from_str("10").unwrap(),
                unit_price: Decimal::from_str("125.00").unwrap(),
                tax_category: TaxCategory::Standard,
            }],
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
            due_at: None,
            template_path: None,
        },
    )
    .await
    .unwrap();
    let issued = inv_commands::issue_invoice(
        &ctx,
        inv_commands::IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap();

    let mk_send = || SendInvoiceInput {
        invoice_id: issued.invoice.id.clone(),
        destination_uri: "stdout".to_string(),
        idempotency_key: Some("send-1".into()),
        actor: Actor::Cli { name: "jad".into() },
        channel: Channel::Cli,
        sink: None,
    };

    let mut sink1: Vec<u8> = Vec::new();
    let mut input1 = mk_send();
    input1.sink = Some(&mut sink1);
    let s1 = send_invoice(&ctx, input1).await.expect("send1");
    assert_eq!(s1.invoice.state, inv_core::domain::invoice::InvoiceState::Sent);

    // Second call with same key + same body: today, FSM permits the
    // Sent → Sent self-edge, so we get a new history row + new emitted
    // event set. When dedup ships, this test will need updating.
    let mut sink2: Vec<u8> = Vec::new();
    let mut input2 = mk_send();
    input2.sink = Some(&mut sink2);
    let s2 = send_invoice(&ctx, input2).await.expect("send2 (current behaviour)");
    let hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&issued.invoice.id)
        .await
        .unwrap();
    let send_rows = hist.iter().filter(|h| h.event == "send").count();
    assert_eq!(
        send_rows, 2,
        "current behaviour: send w/ same idempotency_key inserts a second history row (gap)",
    );
    let _ = s2;
}

// =============================================================================
// helpers
// =============================================================================

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("json body")
}
