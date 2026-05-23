//! Story saas-biller-de-03: Stripe payment via fin auto-advances
//! invoice to paid.
//!
//! Surfaces: bus consumer (fin.billing.payment.received → mark_paid),
//! store (invoice state + amount_paid + invoice_state_history +
//! bus_inbox dedup), bus emission (inv.billing.invoice.paid).
//!
//! xrr: used for the inbound fin event payload — adapter `exec` (the
//! synthetic fin emit is modelled as an exec call so the cassette
//! captures the canonical payload bytes). External-boundary fit per the
//! triage: fin is a separate service / binary in production; recording
//! a synthetic emit here pins the payload shape against accidental
//! schema drift in fin.

mod common;

use std::collections::HashMap;
use std::str::FromStr;

use rust_decimal::Decimal;

use hop_top_xrr::adapters::exec::{ExecAdapter, ExecRequest, ExecResponse};

use hop_top_inv_bus::{dispatch_inbound_event, Consumer, DispatchOutput};
use hop_top_inv_commands::{
    draft_invoice, issue_invoice, send_invoice, Actor, Channel, DraftInvoiceInput, DraftLineInput,
    IssueInvoiceInput, SendInvoiceInput,
};
use hop_top_inv_core::domain::invoice::{InvoiceState, TaxCategory};
use hop_top_inv_core::domain::jurisdiction::Jurisdiction;
use hop_top_inv_core::domain::money::Currency;
use hop_top_inv_store::repo::bus_inbox::BusInboxRepo;
use hop_top_inv_store::repo::invoice::InvoiceRepo;

const TEST_NAME: &str = "saas_biller_de_03_fin_stripe_payment_paid";

#[tokio::test]
async fn fin_payment_received_advances_invoice_to_paid() {
    let (ctx, pool, captured, _blob) = common::fresh_ctx().await;
    let cust = common::seed_customer_qc(&pool).await;
    let session = common::xrr_session(TEST_NAME);

    // Set up: draft → issue → send, leaving invoice in Sent.
    let drafted = draft_invoice(
        &ctx,
        DraftInvoiceInput {
            customer_id: cust.clone(),
            seller_jurisdiction: Jurisdiction::QuebecCa,
            currency: Currency::CAD,
            lines: vec![
                DraftLineInput {
                    description: "Consulting".into(),
                    quantity: Decimal::from_str("10").unwrap(),
                    unit_price: Decimal::from_str("125.00").unwrap(),
                    tax_category: TaxCategory::Standard,
                },
                DraftLineInput {
                    description: "Travel".into(),
                    quantity: Decimal::from_str("1").unwrap(),
                    unit_price: Decimal::from_str("200.00").unwrap(),
                    tax_category: TaxCategory::Standard,
                },
            ],
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
    let issued = issue_invoice(
        &ctx,
        IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap();
    // total = 1450 + 217.14 = 1667.14
    assert_eq!(issued.invoice.total, Decimal::from_str("1667.14").unwrap());
    let mut sink: Vec<u8> = Vec::new();
    let _ = send_invoice(
        &ctx,
        SendInvoiceInput {
            invoice_id: issued.invoice.id.clone(),
            destination_uri: "stdout".to_string(),
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
            sink: Some(&mut sink),
        },
    )
    .await
    .unwrap();

    // ----- When: fin publishes payment.received (full amount). -------
    // xrr fingerprint = hash(argv + stdin). Invoice id is non-determ
    // typeid → use a placeholder + substitute after the session.
    const PLACEHOLDER_ID: &str = "invoice_INVID_PLACEHOLDER";
    let recorded_payload = serde_json::json!({
        "invoice_ref": format!("inv://invoice/{}", PLACEHOLDER_ID),
        "amount": "1667.14",
        "received_at": "2026-06-15T10:00:00Z",
        "idempotency_key": "fin-payment-stripe-pi_abc"
    })
    .to_string();
    let req = ExecRequest {
        argv: vec!["fin-bus-emit".into(), "fin.billing.payment.received".into()],
        stdin: recorded_payload.clone(),
        env: HashMap::new(),
    };
    let resp = session
        .record(&ExecAdapter, &req, || {
            Ok(ExecResponse {
                stdout: recorded_payload.clone(),
                stderr: String::new(),
                exit_code: 0,
                duration_ms: 1,
            })
        })
        .expect("xrr session");
    assert_eq!(resp.exit_code, 0);
    let event_payload = resp
        .stdout
        .replace(PLACEHOLDER_ID, &issued.invoice.id.to_string());

    let inbox = BusInboxRepo::new(&pool);
    let first = dispatch_inbound_event(
        &inbox,
        "fin-evt-abc-1",
        "fin.billing.payment.received",
        "fin",
        &event_payload,
    )
    .await
    .expect("inbox");
    assert!(first, "first delivery accepted");

    let consumer = Consumer::new();
    let out = consumer
        .dispatch(
            &ctx,
            "fin.billing.payment.received",
            "fin-evt-abc-1",
            &event_payload,
        )
        .await
        .expect("dispatch");
    let paid = match out {
        DispatchOutput::Paid(p) => p,
        other => panic!("expected Paid, got {other:?}"),
    };

    // ----- Then: state=paid, .paid emitted, cumulative=1667.14. ------
    assert!(paid.fully_paid);
    assert_eq!(paid.invoice.state, InvoiceState::Paid);
    assert_eq!(
        paid.invoice.amount_paid,
        Decimal::from_str("1667.14").unwrap()
    );
    let topics = captured.topics();
    assert!(topics.contains(&"inv.billing.invoice.paid".to_string()));

    // ----- Given the same event_id re-delivered. ---------------------
    let second = dispatch_inbound_event(
        &inbox,
        "fin-evt-abc-1",
        "fin.billing.payment.received",
        "fin",
        &event_payload,
    )
    .await
    .expect("inbox replay");
    assert!(!second, "re-delivery must dedup at the inbox");

    let back = InvoiceRepo::new(&pool)
        .get(&issued.invoice.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back.state, InvoiceState::Paid);
}

#[tokio::test]
async fn fin_partial_payment_then_remainder_reaches_paid() {
    let (ctx, pool, _captured, _blob) = common::fresh_ctx().await;
    let cust = common::seed_customer_qc(&pool).await;

    // Same draft as the full-payment test.
    let drafted = draft_invoice(
        &ctx,
        DraftInvoiceInput {
            customer_id: cust.clone(),
            seller_jurisdiction: Jurisdiction::QuebecCa,
            currency: Currency::CAD,
            lines: vec![DraftLineInput {
                description: "Consulting".into(),
                quantity: Decimal::from_str("10").unwrap(),
                unit_price: Decimal::from_str("100.00").unwrap(),
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
    let issued = issue_invoice(
        &ctx,
        IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap();
    let total = issued.invoice.total;
    let half = (total / Decimal::from(2)).round_dp(2);

    let consumer = Consumer::new();
    let p1_payload = serde_json::json!({
        "invoice_ref": format!("inv://invoice/{}", issued.invoice.id),
        "amount": half.to_string(),
        "received_at": "2026-06-15T10:00:00Z",
    })
    .to_string();
    let out1 = consumer
        .dispatch(
            &ctx,
            "fin.billing.payment.received",
            "fin-partial-1",
            &p1_payload,
        )
        .await
        .expect("partial 1");
    match out1 {
        DispatchOutput::Paid(p) => {
            assert!(!p.fully_paid);
            assert_eq!(p.invoice.state, InvoiceState::PartiallyPaid);
        }
        other => panic!("expected Paid, got {other:?}"),
    }

    let p2_payload = serde_json::json!({
        "invoice_ref": format!("inv://invoice/{}", issued.invoice.id),
        "amount": (total - half).to_string(),
        "received_at": "2026-06-16T10:00:00Z",
    })
    .to_string();
    let out2 = consumer
        .dispatch(
            &ctx,
            "fin.billing.payment.received",
            "fin-partial-2",
            &p2_payload,
        )
        .await
        .expect("partial 2");
    match out2 {
        DispatchOutput::Paid(p) => {
            assert!(p.fully_paid);
            assert_eq!(p.invoice.state, InvoiceState::Paid);
            assert_eq!(p.invoice.amount_paid, total);
        }
        other => panic!("expected Paid, got {other:?}"),
    }
}
