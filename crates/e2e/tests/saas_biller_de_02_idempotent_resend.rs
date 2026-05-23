//! Story saas-biller-de-02: idempotent re-send does not double-publish.
//!
//! Surfaces: HTTP API, commands (idempotency_key dedup),
//! store (UNIQUE constraint + no duplicate history row),
//! bus outbox (no double-emit on replay).
//!
//! ## Implementation status (T-0037)
//!
//! `idempotency_key` dedup at the command layer is now implemented on
//! BOTH `draft_invoice` and `send_invoice` / `send_invoice_render`.
//! Send-level dedup is scoped per-(invoice_id, idempotency_key) via
//! the `send_idempotency` table. On a hit, the command returns the
//! same shape with `idempotency_replay: true`, no FSM mutation, no
//! second `send` history row, no duplicate `inv.billing.invoice.sent`
//! event.

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
    let (ctx, pool, captured, _blob) = common::fresh_ctx().await;
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
        schedule_id: None,
    };
    let first = draft_invoice(&ctx, input()).await.expect("first");
    let topics_after_first = captured.topics();
    let second = draft_invoice(&ctx, input()).await.expect("second");
    assert_eq!(first.invoice.id, second.invoice.id);
    assert!(!first.idempotency_replay);
    assert!(second.idempotency_replay, "second call must replay");
    let topics_after_second = captured.topics();
    assert_eq!(
        topics_after_second, topics_after_first,
        "replay must NOT emit; before={topics_after_first:?} after={topics_after_second:?}",
    );
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
async fn send_idempotency_key_replays() {
    // T-0037: send_invoice now honours idempotency_key.
    // Second call with the same (invoice_id, idempotency_key):
    //   - returns the same shape with `idempotency_replay: true`
    //   - does NOT advance the FSM a second time (no extra history row)
    //   - does NOT write to the sink
    //   - does NOT emit any events (incl. `inv.billing.invoice.sent`)
    let (ctx, pool, captured, _blob) = common::fresh_ctx().await;
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
            schedule_id: None,
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

    fn mk_send<'s>(
        invoice_id: inv_core::domain::ids::InvoiceId,
        sink: &'s mut dyn inv_commands::SendSink,
    ) -> SendInvoiceInput<'s> {
        SendInvoiceInput {
            invoice_id,
            destination_uri: "stdout".to_string(),
            idempotency_key: Some("send-1".into()),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
            sink: Some(sink),
        }
    }

    let mut sink1: Vec<u8> = Vec::new();
    let s1 = send_invoice(&ctx, mk_send(issued.invoice.id.clone(), &mut sink1))
        .await
        .expect("send1");
    assert_eq!(
        s1.invoice.state,
        inv_core::domain::invoice::InvoiceState::Sent
    );
    assert!(!s1.idempotency_replay, "first call must NOT be a replay");
    let s1_delivered = s1.delivered_to.clone();
    drop(s1);
    assert!(!sink1.is_empty(), "first call writes bytes to the sink");
    let sink1_len = sink1.len();
    let topics_after_first_send = captured.topics();

    // Second call with same key: must replay, no new history row, no
    // sink write, no events.
    let mut sink2: Vec<u8> = Vec::new();
    let s2 = send_invoice(&ctx, mk_send(issued.invoice.id.clone(), &mut sink2))
        .await
        .expect("send2 (replay)");
    assert!(s2.idempotency_replay, "second call must replay");
    let topics_after_second_send = captured.topics();
    assert_eq!(
        topics_after_second_send, topics_after_first_send,
        "replay must NOT emit; before={topics_after_first_send:?} after={topics_after_second_send:?}",
    );
    assert_eq!(s2.delivered_to, s1_delivered, "delivered_to preserved");
    drop(s2);
    assert!(sink2.is_empty(), "replay must NOT write to the sink");

    let hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&issued.invoice.id)
        .await
        .unwrap();
    let send_rows = hist.iter().filter(|h| h.event == "send").count();
    assert_eq!(
        send_rows, 1,
        "replay must not insert a second send history row"
    );

    // Sink1 sanity — len unchanged after replay (closing the loop).
    assert_eq!(sink1.len(), sink1_len);
}

// =============================================================================
// helpers
// =============================================================================

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).expect("json body")
}
