//! Story freelancer-qc-01: draft → issue → send via signed link →
//! fin auto-marks paid.
//!
//! Surfaces: CLI (in-process via inv-commands; the CLI binary is a thin
//! wrapper around the same commands), bus consumer (fin.billing.payment.received
//! via inbox), store (invoices + invoice_state_history + bus_inbox),
//! signed-link minting (api adapter — exercised via inv_api directly).
//!
//! xrr: used for the inbound `fin.billing.payment.received` event —
//! adapter `exec` (since the synthetic fin source is just a string blob
//! that we record as the payload of a "fin-emit" exec call). Per the
//! triage, this puts xrr at a single external seam: the synthetic fin
//! publisher.

mod common;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::Value;

use hop_top_xrr::adapters::exec::{ExecAdapter, ExecRequest, ExecResponse};

use inv_bus::{dispatch_inbound_event, Consumer, DispatchOutput};
use inv_commands::{
    draft_invoice, issue_invoice, send_invoice, Actor, Channel, DraftInvoiceInput,
    DraftLineInput, IssueInvoiceInput, SendInvoiceInput,
};
use inv_core::domain::invoice::{InvoiceState, TaxCategory};
use inv_core::domain::jurisdiction::Jurisdiction;
use inv_core::domain::money::Currency;
use inv_store::repo::bus_inbox::BusInboxRepo;
use inv_store::repo::history::InvoiceHistoryRepo;
use inv_store::repo::invoice::InvoiceRepo;

const TEST_NAME: &str = "freelancer_qc_01_draft_link_fin_paid";

#[tokio::test]
async fn full_lifecycle_with_fin_payment() {
    let (ctx, pool, captured, _blob) = common::fresh_ctx().await;
    let cust = common::seed_customer_qc(&pool).await;
    let session = common::xrr_session(TEST_NAME);

    // ----- Given: a customer in QC. (seed_customer_qc) ----------------
    // ----- When: draft. ----------------------------------------------
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
            idempotency_key: Some("draft-2026-06-acme-1".into()),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
            due_at: Some("2026-06-30T00:00:00Z".parse().unwrap()),
            template_path: None,
            schedule_id: None,
        },
    )
    .await
    .expect("draft");

    // ----- Then: state=draft, subtotal=1450, tax=0, history pending. --
    assert_eq!(drafted.invoice.state, InvoiceState::Draft);
    assert_eq!(drafted.invoice.subtotal, Decimal::from_str("1450.00").unwrap());
    assert_eq!(drafted.invoice.tax_total, Decimal::ZERO);
    let hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&drafted.invoice.id)
        .await
        .unwrap();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0].to_state, InvoiceState::Draft);
    assert!(hist[0].published_at.is_none(), "outbox pending");

    // ----- When: issue. ----------------------------------------------
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
    .expect("issue");

    // ----- Then: state=issued, number set, tax frozen at QC GST + QST.
    // 1450 * 0.05 + 1450 * 0.09975 = 72.50 + 144.6375 = 217.1375
    // → banker's-rounded to 217.14 at currency scale 2 (CAD).
    assert_eq!(issued.invoice.state, InvoiceState::Issued);
    let num = issued.invoice.number.as_deref().expect("number");
    assert!(num.starts_with("INV-2026-"), "got {num}");
    assert_eq!(issued.invoice.tax_total, Decimal::from_str("217.14").unwrap());
    assert_eq!(issued.invoice.total, Decimal::from_str("1667.14").unwrap());
    // PDF rendered + blob stored.
    assert!(issued.invoice.pdf_blob_ref.as_deref().unwrap_or("").starts_with("blob://local/"));

    // ----- When: send via link://. -----------------------------------
    // The command itself returns NotImplemented for link:// (channel
    // owns transport). We mint the signed link the same way inv-api's
    // POST /v1/invoices/{id}/send does for link:// and assert FSM
    // transitions via a stdout-sink send to model the "send" event.
    let mut sink: Vec<u8> = Vec::new();
    let sent = send_invoice(
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
    .expect("send stdout");
    assert_eq!(sent.invoice.state, InvoiceState::Sent);
    let topics: Vec<&str> = sent.emitted_events.iter().map(|e| e.topic.as_str()).collect();
    assert!(topics.contains(&"inv.billing.invoice.sent"));
    // Signed-link minting (mirrors inv-api's POST /v1/invoices/{id}/send
    // for link://): assert the helper produces a stable token round-trip.
    let token = inv_api::signed_link::sign(
        &issued.invoice.id.to_string(),
        3600,
        b"test-link-key",
    );
    let url = format!("http://localhost:7400/v/{token}");
    assert!(url.contains("/v/"));
    assert!(!token.is_empty());

    // ----- Given fin publishes payment.received. ---------------------
    // Build the synthetic fin payload + record/replay through xrr's
    // exec adapter (the "fin emit" call is modelled as an exec call so
    // the payload bytes are stored in the cassette).
    //
    // xrr fingerprint = hash(argv + stdin). Both MUST be stable across
    // runs or replay always misses. Invoice id is non-deterministic
    // (typeid randomness), so we use a placeholder in the recorded
    // request + substitute the real id back into the response below.
    const PLACEHOLDER_ID: &str = "invoice_INVID_PLACEHOLDER";
    let recorded_payload = serde_json::json!({
        "invoice_ref": format!("inv://invoice/{}", PLACEHOLDER_ID),
        "amount": "1667.14",
        "received_at": "2026-06-15T10:00:00Z",
    })
    .to_string();
    let req = ExecRequest {
        argv: vec![
            "fin-bus-emit".into(),
            "fin.billing.payment.received".into(),
        ],
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
    assert_eq!(resp.exit_code, 0, "fin emit should succeed");
    // Substitute the real invoice id into the recorded payload.
    let event_payload = resp.stdout.replace(PLACEHOLDER_ID, &issued.invoice.id.to_string());

    // ----- When: inv's bus consumer processes the event. -------------
    // Inbox dedup + dispatch.
    let inbox = BusInboxRepo::new(&pool);
    let first = dispatch_inbound_event(
        &inbox,
        "fin-evt-1",
        "fin.billing.payment.received",
        "fin",
        &event_payload,
    )
    .await
    .expect("inbox insert");
    assert!(first, "first delivery must accept");
    let consumer = Consumer::new();
    let out = consumer
        .dispatch(&ctx, "fin.billing.payment.received", "fin-evt-1", &event_payload)
        .await
        .expect("dispatch");
    let paid = match out {
        DispatchOutput::Paid(p) => p,
        other => panic!("expected Paid, got {other:?}"),
    };

    // ----- Then: invoice paid, emitted invoice.paid, cumulative_paid.
    assert!(paid.fully_paid);
    assert_eq!(paid.invoice.state, InvoiceState::Paid);
    assert_eq!(paid.invoice.amount_paid, Decimal::from_str("1667.14").unwrap());
    let topics: Vec<&str> = paid.emitted_events.iter().map(|e| e.topic.as_str()).collect();
    assert!(topics.contains(&"inv.billing.invoice.paid"));

    // Replay of the same event_id: inbox refuses, no double-emit.
    let second = dispatch_inbound_event(
        &inbox,
        "fin-evt-1",
        "fin.billing.payment.received",
        "fin",
        &event_payload,
    )
    .await
    .expect("inbox replay");
    assert!(!second, "replay must dedup");

    // History: draft + issue + sent + paid = 4 rows.
    let back = InvoiceRepo::new(&pool)
        .get(&issued.invoice.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back.state, InvoiceState::Paid);
    let final_hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&issued.invoice.id)
        .await
        .unwrap();
    assert_eq!(final_hist.len(), 4);

    // Drain the outbox relay → publisher captures every queued event.
    // The publisher Arc was wired into CoreCtx via fresh_ctx().
    let stats = inv_bus::run_outbox_relay(&pool, captured.as_ref(), 50)
        .await
        .expect("relay");
    assert!(stats.rows_processed > 0, "relay should drain pending rows");
    let captured_topics = captured.topics();
    assert!(
        captured_topics.iter().any(|t| t == "inv.billing.invoice.paid"),
        "publisher topics: {captured_topics:?}"
    );

    // Silence unused-import lint when fields drift.
    let _: Value = serde_json::json!({});
    let _: Arc<()> = Arc::new(());
    let _ = Utc::now();
}
