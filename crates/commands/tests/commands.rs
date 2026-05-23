//! Integration tests for the hop-top-inv-commands one-command-core layer.
//!
//! Each test opens an in-memory sqlite pool, applies the hop-top-inv-store
//! migrations, seeds a customer, and exercises one or more of
//! `draft_invoice` / `issue_invoice` / `send_invoice`. The dev-dep
//! `hop-top-inv-store` is configured with the `sqlite` feature, so the in-memory
//! pool always works without a cfg guard.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;

use hop_top_inv_core::domain::address::Address;
use hop_top_inv_core::domain::customer::Customer;
use hop_top_inv_core::domain::ids::CustomerId;
use hop_top_inv_core::domain::invoice::{InvoiceState, TaxCategory};
use hop_top_inv_core::domain::jurisdiction::Jurisdiction;
use hop_top_inv_core::domain::money::Currency;
use hop_top_inv_core::state::TransitionError;
use hop_top_inv_core::tax::TaxTable;

use hop_top_inv_bus::InMemoryPublisher;
use hop_top_inv_commands::{
    create_credit_note, draft_invoice, issue_credit_note, issue_invoice, mark_overdue_ticker,
    mark_paid, reminder_cancel, reminder_schedule, reminders_tick, schedule_cancel,
    schedule_create, schedule_pause, schedules_tick, send_invoice, void_invoice, Actor, Channel,
    Clock, CoreCtx, CoreError, CreateCreditNoteInput, DraftInvoiceInput, DraftLineInput,
    IssueCreditNoteInput, IssueInvoiceInput, MarkPaidInput, Publisher, ReminderCancelInput,
    ReminderScheduleInput, ScheduleCreateInput, ScheduleLineInput, ScheduleStateChangeInput,
    SendInvoiceInput, VoidInvoiceInput,
};
use hop_top_inv_core::domain::creditnote::CreditNoteState;
use hop_top_inv_core::domain::reminder::{ReminderChannel, ReminderState};
use hop_top_inv_core::domain::schedule::{Cadence, ScheduleState};
use hop_top_inv_store::blob::LocalBlobStore;
use hop_top_inv_store::connect_for_tests;
use hop_top_inv_store::pool::Pool;
use hop_top_inv_store::repo::history::InvoiceHistoryRepo;
use hop_top_inv_store::repo::{CreditNoteRepo, CustomerRepo, InvoiceRepo};

/// Frozen-clock impl for deterministic tests.
struct FrozenClock(DateTime<Utc>);

impl Clock for FrozenClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// Inline tax-table fixture covering QC seller + CA-QC buyer (the
/// happy-path scenario used by issue + send tests).
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

async fn fresh_ctx() -> (CoreCtx, Pool, Arc<InMemoryPublisher>, tempfile::TempDir) {
    let (ctx, pool, publisher, blob_dir) = fresh_ctx_inner(true).await;
    (
        ctx,
        pool,
        publisher,
        blob_dir.expect("blob dir present when blob store wired"),
    )
}

/// Variant that returns a ctx with `blob_store = None`. Used by the
/// test that asserts the no-blob-store path leaves `pdf_blob_ref` unset.
async fn fresh_ctx_no_blob() -> (CoreCtx, Pool, Arc<InMemoryPublisher>) {
    let (ctx, pool, publisher, _) = fresh_ctx_inner(false).await;
    (ctx, pool, publisher)
}

async fn fresh_ctx_inner(
    with_blob: bool,
) -> (
    CoreCtx,
    Pool,
    Arc<InMemoryPublisher>,
    Option<tempfile::TempDir>,
) {
    let pool = connect_for_tests().await.expect("connect_for_tests");
    let (table, nexus) = TaxTable::load_from_str(FIXTURE_TOML).expect("tax fixture");
    let publisher: Arc<InMemoryPublisher> = Arc::new(InMemoryPublisher::new());
    let mut ctx = CoreCtx::new(pool.clone(), table, nexus)
        .with_clock(Arc::new(FrozenClock(frozen_now())))
        .with_publisher(publisher.clone() as Arc<dyn Publisher>);
    let blob_dir = if with_blob {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LocalBlobStore::new(dir.path(), b"test-signing-key".to_vec())
            .expect("local blob store");
        ctx = ctx.with_blob_store(Arc::new(store));
        Some(dir)
    } else {
        None
    };
    (ctx, pool, publisher, blob_dir)
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

fn draft_input(customer_id: &CustomerId, idempotency: Option<&str>) -> DraftInvoiceInput {
    DraftInvoiceInput {
        customer_id: customer_id.clone(),
        seller_jurisdiction: Jurisdiction::QuebecCa,
        currency: Currency::CAD,
        lines: vec![DraftLineInput {
            description: "Consulting hours".into(),
            quantity: Decimal::from_str("10").unwrap(),
            unit_price: Decimal::from_str("125.00").unwrap(),
            tax_category: TaxCategory::Standard,
        }],
        idempotency_key: idempotency.map(|s| s.to_string()),
        actor: Actor::Cli { name: "jad".into() },
        channel: Channel::Cli,
        due_at: None,
        template_path: None,
        schedule_id: None,
    }
}

// ---------------------------------------------------------------------
// draft_invoice
// ---------------------------------------------------------------------

#[tokio::test]
async fn draft_happy_path_creates_draft_with_computed_totals() {
    let (ctx, pool, publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;

    let out = draft_invoice(&ctx, draft_input(&cust, None))
        .await
        .expect("draft ok");

    assert_eq!(out.invoice.state, InvoiceState::Draft);
    assert_eq!(out.invoice.customer_id, cust);
    assert_eq!(out.invoice.currency, Currency::CAD);
    // Subtotal = 10 * 125 = 1250.00, tax frozen at issue so 0.00 here.
    assert_eq!(out.invoice.subtotal, Decimal::from_str("1250.00").unwrap());
    assert_eq!(out.invoice.tax_total, Decimal::ZERO);
    assert_eq!(out.invoice.total, Decimal::from_str("1250.00").unwrap());
    assert!(out.invoice.number.is_none(), "drafts have no number");
    assert!(out.invoice.issued_at.is_none());
    assert_eq!(out.lines.len(), 1);
    assert!(!out.idempotency_replay);

    // emitted: inv.billing.invoice.drafted via the InMemoryPublisher
    // captured into ctx (T-0043).
    let captured = publisher.captured();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].topic, "inv.billing.invoice.drafted");

    // A history row was written (audit + outbox).
    let hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&out.invoice.id)
        .await
        .unwrap();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0].to_state, InvoiceState::Draft);
    assert_eq!(hist[0].event, "draft");
    assert!(hist[0].published_at.is_none(), "outbox pending");
}

#[tokio::test]
async fn draft_idempotency_returns_existing_invoice() {
    let (ctx, pool, publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;

    let first = draft_invoice(&ctx, draft_input(&cust, Some("key-1")))
        .await
        .unwrap();
    let second = draft_invoice(&ctx, draft_input(&cust, Some("key-1")))
        .await
        .unwrap();

    assert_eq!(first.invoice.id, second.invoice.id);
    assert!(!first.idempotency_replay);
    assert!(second.idempotency_replay);
    // Replay must NOT publish a second drafted event — only the first
    // call should have hit the publisher.
    let topics = publisher.topics();
    assert_eq!(
        topics
            .iter()
            .filter(|t| *t == "inv.billing.invoice.drafted")
            .count(),
        1,
        "replay must not emit; captured: {topics:?}"
    );

    // Still only one history row.
    let hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&first.invoice.id)
        .await
        .unwrap();
    assert_eq!(hist.len(), 1);
}

#[tokio::test]
async fn draft_validation_rejects_empty_lines() {
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let mut input = draft_input(&cust, None);
    input.lines.clear();

    let err = draft_invoice(&ctx, input).await.unwrap_err();
    match err {
        CoreError::Validation(msg) => assert!(msg.contains("at least one line")),
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[tokio::test]
async fn draft_validation_rejects_unsupported_currency() {
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let mut input = draft_input(&cust, None);
    input.currency = Currency::new("EUR").unwrap();

    let err = draft_invoice(&ctx, input).await.unwrap_err();
    match err {
        CoreError::Validation(msg) => assert!(msg.contains("EUR")),
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[tokio::test]
async fn draft_validation_rejects_zero_quantity() {
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let mut input = draft_input(&cust, None);
    input.lines[0].quantity = Decimal::ZERO;

    let err = draft_invoice(&ctx, input).await.unwrap_err();
    match err {
        CoreError::Validation(msg) => assert!(msg.contains("quantity")),
        other => panic!("expected Validation, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// issue_invoice
// ---------------------------------------------------------------------

#[tokio::test]
async fn issue_happy_path_freezes_tax_and_assigns_number() {
    let (ctx, pool, publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();

    let out = issue_invoice(
        &ctx,
        IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("issue ok");

    assert_eq!(out.invoice.state, InvoiceState::Issued);
    assert_eq!(out.invoice.number.as_deref(), Some("INV-2026-0001"));
    assert!(out.invoice.issued_at.is_some());
    // GST 5% + QST 9.975% on 1250 = 187.1875 → banker's-round to 187.19.
    assert_eq!(out.invoice.tax_total, Decimal::from_str("187.19").unwrap());
    assert_eq!(out.invoice.total, Decimal::from_str("1437.19").unwrap());
    assert_eq!(out.lines.len(), 1);
    assert_eq!(out.lines[0].tax_rate_ids.len(), 2);
    assert!(!out.html.is_empty());
    assert!(!out.pdf.is_empty());

    // Blob persistence (T-0025): with a LocalBlobStore wired into the
    // CoreCtx, issue must persist the rendered PDF and stamp the
    // invoice with the resulting `blob://local/...` URI.
    let blob_uri = out
        .invoice
        .pdf_blob_ref
        .as_deref()
        .expect("pdf_blob_ref set when blob_store is configured");
    assert!(
        blob_uri.starts_with("blob://local/"),
        "expected blob://local/... uri, got {blob_uri}"
    );

    // Mechanic-triplet + domain `.issued` published synchronously
    // (T-0043). The drafted event from the prior call also lives in
    // the same publisher's capture buffer.
    let topics = publisher.topics();
    assert!(topics.contains(&"inv.billing.invoice.proposed".to_string()));
    assert!(topics.contains(&"inv.billing.invoice.transitioned".to_string()));
    assert!(topics.contains(&"inv.billing.invoice.entered".to_string()));
    assert!(topics.contains(&"inv.billing.invoice.issued".to_string()));

    // history row written.
    let hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&out.invoice.id)
        .await
        .unwrap();
    // draft + issue.
    assert_eq!(hist.len(), 2);
    assert_eq!(hist[1].to_state, InvoiceState::Issued);
    assert_eq!(hist[1].event, "issue");
    assert!(hist[1].published_at.is_none(), "outbox pending");
}

#[tokio::test]
async fn issue_on_already_issued_returns_fsm_error() {
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();
    issue_invoice(
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

    let err = issue_invoice(
        &ctx,
        IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap_err();

    match err {
        CoreError::FsmTransition(TransitionError::Illegal { event: "issue", .. }) => {}
        other => panic!("expected FsmTransition(Illegal{{issue}}), got {other:?}"),
    }
}

#[tokio::test]
async fn issue_without_blob_store_leaves_pdf_blob_ref_unset() {
    // T-0025: when CoreCtx.blob_store is None the command must succeed,
    // return the PDF bytes in the output, and leave `pdf_blob_ref`
    // untouched (`None`). This documents the graceful-degradation path
    // used by lightweight harnesses / adapters that don't wire a blob
    // backend.
    let (ctx, pool, _publisher) = fresh_ctx_no_blob().await;
    assert!(ctx.blob_store.is_none(), "precondition: no blob store");
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();

    let out = issue_invoice(
        &ctx,
        IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("issue ok without blob store");

    assert_eq!(out.invoice.state, InvoiceState::Issued);
    assert!(!out.pdf.is_empty(), "bytes still returned to caller");
    assert!(
        out.invoice.pdf_blob_ref.is_none(),
        "no blob store wired -> pdf_blob_ref must stay None"
    );
}

// ---------------------------------------------------------------------
// send_invoice
// ---------------------------------------------------------------------

#[tokio::test]
async fn send_file_writes_bytes_and_transitions_to_sent() {
    let (ctx, pool, publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();
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

    let dir = std::env::temp_dir().join(format!(
        "hop-top-inv-commands-test-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let target = dir.join("invoice.html");
    let uri = format!("file://{}", target.display());

    let out = send_invoice(
        &ctx,
        SendInvoiceInput {
            invoice_id: issued.invoice.id.clone(),
            destination_uri: uri.clone(),
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
            sink: None,
        },
    )
    .await
    .expect("send ok");

    assert_eq!(out.invoice.state, InvoiceState::Sent);
    assert!(out.invoice.sent_at.is_some());
    assert!(out.delivered_to.starts_with("file://"));
    assert!(target.exists(), "destination file should exist");
    let bytes = std::fs::read(&target).unwrap();
    assert_eq!(bytes, out.pdf);

    // Cleanup.
    let _ = std::fs::remove_file(&target);
    let _ = std::fs::remove_dir(&dir);

    // Topics fired via the InMemoryPublisher captured into ctx.
    let topics = publisher.topics();
    assert!(topics.contains(&"inv.billing.invoice.sent".to_string()));
    assert!(topics.contains(&"inv.billing.invoice.transitioned".to_string()));

    // history row for "send".
    let hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&out.invoice.id)
        .await
        .unwrap();
    assert!(hist.iter().any(|h| h.event == "send"));
}

#[tokio::test]
async fn send_stdout_writes_to_injected_sink() {
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();
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

    let mut sink: Vec<u8> = Vec::new();
    let out = send_invoice(
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
    .expect("send ok");

    assert_eq!(out.invoice.state, InvoiceState::Sent);
    assert_eq!(out.delivered_to, "stdout");
    assert!(!sink.is_empty(), "sink should have received bytes");
    assert_eq!(sink, out.pdf);
}

#[tokio::test]
async fn send_unsupported_scheme_returns_not_implemented() {
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();
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

    let err = send_invoice(
        &ctx,
        SendInvoiceInput {
            invoice_id: issued.invoice.id.clone(),
            destination_uri: "bus://inv.billing".to_string(),
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
            sink: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, CoreError::NotImplemented(_)));
}

// ---------------------------------------------------------------------
// send_invoice idempotency (T-0037)
// ---------------------------------------------------------------------

/// Helper: walk draft → issue and return the issued invoice id, ready
/// for a send call.
async fn drafted_then_issued(
    ctx: &CoreCtx,
    pool: &Pool,
) -> hop_top_inv_core::domain::ids::InvoiceId {
    let cust = seed_customer(pool).await;
    let drafted = draft_invoice(ctx, draft_input(&cust, None)).await.unwrap();
    let issued = issue_invoice(
        ctx,
        IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap();
    issued.invoice.id
}

#[tokio::test]
async fn send_idempotency_same_key_replays_no_mutation() {
    // T-0037: second call with same (invoice_id, idempotency_key)
    // returns the same shape with `idempotency_replay: true`, does NOT
    // write a second history row, does NOT emit events, does NOT push
    // bytes to the sink.
    let (ctx, pool, publisher, _blob_dir) = fresh_ctx().await;
    let invoice_id = drafted_then_issued(&ctx, &pool).await;

    let mut sink1: Vec<u8> = Vec::new();
    let out1 = send_invoice(
        &ctx,
        SendInvoiceInput {
            invoice_id: invoice_id.clone(),
            destination_uri: "stdout".to_string(),
            idempotency_key: Some("send-key-1".into()),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
            sink: Some(&mut sink1),
        },
    )
    .await
    .expect("first send");
    assert_eq!(out1.invoice.state, InvoiceState::Sent);
    assert!(!out1.idempotency_replay);
    assert!(!sink1.is_empty(), "first send writes the rendered bytes");
    // Capture the topics published so far so we can assert the second
    // (replay) call doesn't add more.
    let topics_after_first = publisher.topics();
    assert!(
        topics_after_first.contains(&"inv.billing.invoice.sent".to_string()),
        "first send emits invoice.sent (captured: {topics_after_first:?})",
    );
    let first_delivered = out1.delivered_to.clone();

    let mut sink2: Vec<u8> = Vec::new();
    let out2 = send_invoice(
        &ctx,
        SendInvoiceInput {
            invoice_id: invoice_id.clone(),
            destination_uri: "stdout".to_string(),
            idempotency_key: Some("send-key-1".into()),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
            sink: Some(&mut sink2),
        },
    )
    .await
    .expect("second send (replay)");

    assert!(
        out2.idempotency_replay,
        "replay flag is set on the second call"
    );
    let topics_after_second = publisher.topics();
    assert_eq!(
        topics_after_second, topics_after_first,
        "replay must not emit additional events; before={topics_after_first:?} after={topics_after_second:?}",
    );
    assert_eq!(out2.delivered_to, first_delivered);
    assert_eq!(out2.invoice.state, InvoiceState::Sent);
    assert!(sink2.is_empty(), "replay must not push bytes to the sink");

    let hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&invoice_id)
        .await
        .unwrap();
    let send_rows = hist.iter().filter(|h| h.event == "send").count();
    assert_eq!(send_rows, 1, "no second send history row on replay");
}

#[tokio::test]
async fn send_idempotency_different_key_proceeds() {
    // Different key on same invoice: NOT a replay. The Sent → Sent
    // self-edge runs, a second `send` history row + event set lands.
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let invoice_id = drafted_then_issued(&ctx, &pool).await;

    let mut sink1: Vec<u8> = Vec::new();
    let out1 = send_invoice(
        &ctx,
        SendInvoiceInput {
            invoice_id: invoice_id.clone(),
            destination_uri: "stdout".to_string(),
            idempotency_key: Some("key-a".into()),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
            sink: Some(&mut sink1),
        },
    )
    .await
    .unwrap();
    assert!(!out1.idempotency_replay);

    let mut sink2: Vec<u8> = Vec::new();
    let out2 = send_invoice(
        &ctx,
        SendInvoiceInput {
            invoice_id: invoice_id.clone(),
            destination_uri: "stdout".to_string(),
            idempotency_key: Some("key-b".into()),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
            sink: Some(&mut sink2),
        },
    )
    .await
    .unwrap();
    assert!(
        !out2.idempotency_replay,
        "different key on same invoice must NOT replay"
    );
    assert!(!sink2.is_empty(), "different key still writes bytes");

    let hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&invoice_id)
        .await
        .unwrap();
    let send_rows = hist.iter().filter(|h| h.event == "send").count();
    assert_eq!(send_rows, 2, "two distinct sends -> two history rows");
}

#[tokio::test]
async fn send_idempotency_key_scoped_per_invoice() {
    // Same key, different invoices: NOT a collision. Each invoice has
    // its own idempotency surface.
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let invoice_a = drafted_then_issued(&ctx, &pool).await;
    let invoice_b = drafted_then_issued(&ctx, &pool).await;
    assert_ne!(invoice_a, invoice_b, "precondition: two distinct invoices");

    let mut sink_a: Vec<u8> = Vec::new();
    let out_a = send_invoice(
        &ctx,
        SendInvoiceInput {
            invoice_id: invoice_a.clone(),
            destination_uri: "stdout".to_string(),
            idempotency_key: Some("shared-key".into()),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
            sink: Some(&mut sink_a),
        },
    )
    .await
    .unwrap();
    assert!(!out_a.idempotency_replay);

    let mut sink_b: Vec<u8> = Vec::new();
    let out_b = send_invoice(
        &ctx,
        SendInvoiceInput {
            invoice_id: invoice_b.clone(),
            destination_uri: "stdout".to_string(),
            idempotency_key: Some("shared-key".into()),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
            sink: Some(&mut sink_b),
        },
    )
    .await
    .unwrap();
    assert!(
        !out_b.idempotency_replay,
        "same key on different invoice must NOT replay"
    );
    assert!(!sink_b.is_empty());

    // Each invoice has exactly one send history row.
    for id in [&invoice_a, &invoice_b] {
        let hist = InvoiceHistoryRepo::new(&pool)
            .list_for_invoice(id)
            .await
            .unwrap();
        let send_rows = hist.iter().filter(|h| h.event == "send").count();
        assert_eq!(send_rows, 1, "one send per invoice (key is per-invoice)");
    }
}

// ---------------------------------------------------------------------
// mark_paid (T-0012)
// ---------------------------------------------------------------------

#[tokio::test]
async fn mark_paid_full_settles_invoice() {
    let (ctx, pool, publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();
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

    let out = mark_paid(
        &ctx,
        MarkPaidInput {
            invoice_id: issued.invoice.id.clone(),
            amount: issued.invoice.total,
            received_at: None,
            idempotency_key: None,
            bus_event_id: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("mark_paid ok");

    assert!(out.fully_paid);
    assert_eq!(out.invoice.state, InvoiceState::Paid);
    assert_eq!(out.invoice.amount_paid, issued.invoice.total);
    assert!(out.invoice.paid_at.is_some());

    let topics = publisher.topics();
    assert!(topics.contains(&"inv.billing.invoice.paid".to_string()));

    // history grew: draft + issue + paid
    let hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&issued.invoice.id)
        .await
        .unwrap();
    assert_eq!(hist.len(), 3);
    assert_eq!(hist[2].to_state, InvoiceState::Paid);
}

#[tokio::test]
async fn mark_paid_partial_then_remainder_reaches_paid() {
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();
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

    let p1 = mark_paid(
        &ctx,
        MarkPaidInput {
            invoice_id: issued.invoice.id.clone(),
            amount: half,
            received_at: None,
            idempotency_key: None,
            bus_event_id: Some("evt-1".into()),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Bus,
        },
    )
    .await
    .unwrap();
    assert!(!p1.fully_paid);
    assert_eq!(p1.invoice.state, InvoiceState::PartiallyPaid);

    let p2 = mark_paid(
        &ctx,
        MarkPaidInput {
            invoice_id: issued.invoice.id.clone(),
            amount: total - half,
            received_at: None,
            idempotency_key: None,
            bus_event_id: Some("evt-2".into()),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Bus,
        },
    )
    .await
    .unwrap();
    assert!(p2.fully_paid);
    assert_eq!(p2.invoice.state, InvoiceState::Paid);
    assert_eq!(p2.invoice.amount_paid, total);

    // history: draft + issue + partial + paid
    let hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&issued.invoice.id)
        .await
        .unwrap();
    assert_eq!(hist.len(), 4);
    assert_eq!(hist[2].to_state, InvoiceState::PartiallyPaid);
    assert_eq!(hist[3].to_state, InvoiceState::Paid);
    // bus_event_id provenance recorded.
    assert_eq!(hist[2].bus_event_id.as_deref(), Some("evt-1"));
    assert_eq!(hist[3].bus_event_id.as_deref(), Some("evt-2"));
}

#[tokio::test]
async fn mark_paid_rejects_non_positive_amount() {
    let (ctx, _pool, _publisher, _blob_dir) = fresh_ctx().await;
    let err = mark_paid(
        &ctx,
        MarkPaidInput {
            invoice_id: hop_top_inv_core::domain::ids::InvoiceId::new(),
            amount: Decimal::ZERO,
            received_at: None,
            idempotency_key: None,
            bus_event_id: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)));
}

// ---------------------------------------------------------------------
// void_invoice (T-0012)
// ---------------------------------------------------------------------

#[tokio::test]
async fn void_pre_payment_succeeds() {
    let (ctx, pool, publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();
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

    let out = void_invoice(
        &ctx,
        VoidInvoiceInput {
            invoice_id: issued.invoice.id.clone(),
            reason: Some("entered in error".into()),
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("void ok");

    assert_eq!(out.invoice.state, InvoiceState::Voided);
    assert!(out.invoice.voided_at.is_some());

    let topics = publisher.topics();
    assert!(topics.contains(&"inv.billing.invoice.voided".to_string()));
}

#[tokio::test]
async fn void_after_payment_rejected_by_fsm() {
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();
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
    mark_paid(
        &ctx,
        MarkPaidInput {
            invoice_id: issued.invoice.id.clone(),
            amount: issued.invoice.total,
            received_at: None,
            idempotency_key: None,
            bus_event_id: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap();

    let err = void_invoice(
        &ctx,
        VoidInvoiceInput {
            invoice_id: issued.invoice.id.clone(),
            reason: None,
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, CoreError::FsmTransition(_)));
}

// ---------------------------------------------------------------------
// credit notes (T-0012)
// ---------------------------------------------------------------------

#[tokio::test]
async fn credit_note_draft_then_issue() {
    let (ctx, pool, publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();
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

    // 1. Draft credit note.
    let cn_draft = create_credit_note(
        &ctx,
        CreateCreditNoteInput {
            invoice_id: issued.invoice.id.clone(),
            amount: Decimal::from_str("100.00").unwrap(),
            reason: Some("duplicate billing".into()),
            refund_ref: Some("refund-evt-1".into()),
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("draft cn ok");

    assert_eq!(cn_draft.credit_note.state, CreditNoteState::Draft);
    assert_eq!(
        cn_draft.credit_note.amount,
        Decimal::from_str("100.00").unwrap()
    );
    assert_eq!(
        cn_draft.credit_note.refund_ref.as_deref(),
        Some("refund-evt-1")
    );
    let topics_after_draft = publisher.topics();
    assert!(topics_after_draft.contains(&"inv.billing.creditnote.drafted".to_string()));

    // 2. Issue credit note.
    let cn_issued = issue_credit_note(
        &ctx,
        IssueCreditNoteInput {
            credit_note_id: cn_draft.credit_note.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("issue cn ok");

    assert_eq!(cn_issued.credit_note.state, CreditNoteState::Issued);
    assert!(cn_issued
        .credit_note
        .number
        .as_deref()
        .unwrap_or("")
        .starts_with("CN-2026-"));
    assert!(cn_issued.credit_note.issued_at.is_some());

    // Verify the cn is in the DB at the issued state.
    let back = CreditNoteRepo::new(&pool)
        .get(&cn_draft.credit_note.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back.state, CreditNoteState::Issued);

    let topics = publisher.topics();
    assert!(topics.contains(&"inv.billing.creditnote.proposed".to_string()));
    assert!(topics.contains(&"inv.billing.creditnote.transitioned".to_string()));
    assert!(topics.contains(&"inv.billing.creditnote.entered".to_string()));
    assert!(topics.contains(&"inv.billing.creditnote.issued".to_string()));
}

#[tokio::test]
async fn credit_note_rejects_non_positive_amount() {
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();
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

    let err = create_credit_note(
        &ctx,
        CreateCreditNoteInput {
            invoice_id: issued.invoice.id.clone(),
            amount: Decimal::ZERO,
            reason: None,
            refund_ref: None,
            idempotency_key: None,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)));
}

// ---------------------------------------------------------------------
// mark_overdue_ticker (T-0012)
// ---------------------------------------------------------------------

#[tokio::test]
async fn overdue_ticker_flags_past_due_invoices() {
    let (ctx, pool, publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();
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

    // Set due_at to a year ago via direct repo write — bypasses the
    // command layer since due_at editing isn't exposed yet.
    let mut inv = issued.invoice.clone();
    inv.due_at = Some(Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap());
    inv.updated_at = inv.due_at.unwrap();
    InvoiceRepo::new(&pool).save(&inv).await.unwrap();

    // Snapshot publisher state pre-tick so we can filter the overdue
    // event out of any noise from prior draft/issue calls.
    let topics_before = publisher.topics();
    let out = mark_overdue_ticker(&ctx).await.expect("tick ok");
    assert_eq!(out.overdue_invoices.len(), 1);
    assert_eq!(out.overdue_invoices[0].id, issued.invoice.id);

    let topics_after = publisher.topics();
    let new_topics: Vec<&str> = topics_after[topics_before.len()..]
        .iter()
        .map(|s| s.as_str())
        .collect();
    assert_eq!(new_topics, vec!["inv.billing.invoice.overdue"]);

    // State unchanged — overdue is a flag, not an FSM state.
    let back = InvoiceRepo::new(&pool)
        .get(&issued.invoice.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back.state, InvoiceState::Issued);
}

#[tokio::test]
async fn overdue_ticker_ignores_unset_due_at() {
    let (ctx, pool, publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();
    // Don't issue — Draft is not in the overdue-eligible set anyway.
    let _ = drafted;
    // Snapshot publisher pre-tick (the draft above emits `.drafted`).
    let topics_before = publisher.topics();
    let out = mark_overdue_ticker(&ctx).await.unwrap();
    assert!(out.overdue_invoices.is_empty());
    assert_eq!(
        publisher.topics(),
        topics_before,
        "overdue tick with no candidates must not publish",
    );
}

// ---------------------------------------------------------------------
// schedules (T-0013)
// ---------------------------------------------------------------------

fn schedule_input(cust: &CustomerId, auto_issue: bool) -> ScheduleCreateInput {
    ScheduleCreateInput {
        customer_id: cust.clone(),
        template_lines: vec![ScheduleLineInput {
            description: "Monthly retainer".into(),
            quantity: Decimal::from_str("1").unwrap(),
            unit_price: Decimal::from_str("500.00").unwrap(),
            tax_category: TaxCategory::Standard,
        }],
        currency: Currency::CAD,
        cadence: Cadence::Monthly { dom: 1 },
        start_date: chrono::NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
        end_date: None,
        auto_issue,
        actor: Actor::Cli { name: "jad".into() },
        channel: Channel::Cli,
    }
}

#[tokio::test]
async fn schedule_create_happy_path() {
    let (ctx, pool, publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;

    let out = schedule_create(&ctx, schedule_input(&cust, false))
        .await
        .expect("create ok");

    assert_eq!(out.schedule.state, ScheduleState::Active);
    assert!(!out.schedule.auto_issue);
    assert_eq!(out.schedule.next_run, out.schedule.start_date);
    assert_eq!(out.schedule.template_lines.len(), 1);
    let topics = publisher.topics();
    assert_eq!(topics, vec!["inv.billing.schedule.created".to_string()]);
}

#[tokio::test]
async fn schedule_validation_rejects_empty_lines() {
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let mut input = schedule_input(&cust, false);
    input.template_lines.clear();
    let err = schedule_create(&ctx, input).await.unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)));
}

#[tokio::test]
async fn schedule_validation_rejects_end_before_start() {
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let mut input = schedule_input(&cust, false);
    input.end_date = Some(chrono::NaiveDate::from_ymd_opt(2026, 5, 1).unwrap());
    let err = schedule_create(&ctx, input).await.unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)));
}

#[tokio::test]
async fn schedule_pause_then_cancel() {
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let created = schedule_create(&ctx, schedule_input(&cust, false))
        .await
        .unwrap();

    let paused = schedule_pause(
        &ctx,
        ScheduleStateChangeInput {
            schedule_id: created.schedule.id.clone(),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap();
    assert_eq!(paused.schedule.state, ScheduleState::Paused);

    let cancelled = schedule_cancel(
        &ctx,
        ScheduleStateChangeInput {
            schedule_id: created.schedule.id.clone(),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap();
    assert_eq!(cancelled.schedule.state, ScheduleState::Cancelled);

    // After cancellation, pausing must reject (terminal).
    let err = schedule_pause(
        &ctx,
        ScheduleStateChangeInput {
            schedule_id: created.schedule.id.clone(),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)));
}

#[tokio::test]
async fn schedules_tick_materialises_drafts_for_due_schedules() {
    // Use a clock past the schedule's start_date so it is due.
    let (mut ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;

    // start_date = 2026-06-01; default frozen_now is 2026-05-19, so the
    // schedule isn't due yet. Bump the clock past start_date.
    ctx = ctx.with_clock(Arc::new(FrozenClock(
        Utc.with_ymd_and_hms(2026, 6, 5, 0, 0, 0).unwrap(),
    )));

    let _created = schedule_create(&ctx, schedule_input(&cust, false))
        .await
        .unwrap();

    let out = schedules_tick(&ctx).await.expect("tick ok");
    assert_eq!(out.ran_schedule_ids.len(), 1);
    assert_eq!(out.drafts.len(), 1);
    assert_eq!(out.drafts[0].invoice.state, InvoiceState::Draft);

    // A second tick on the same day must NOT re-materialise.
    let out2 = schedules_tick(&ctx).await.unwrap();
    assert!(
        out2.ran_schedule_ids.is_empty(),
        "no double-materialisation"
    );
}

#[tokio::test]
async fn schedules_tick_auto_issue_promotes_to_issued() {
    let (mut ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    ctx = ctx.with_clock(Arc::new(FrozenClock(
        Utc.with_ymd_and_hms(2026, 6, 5, 0, 0, 0).unwrap(),
    )));

    let _ = schedule_create(&ctx, schedule_input(&cust, true))
        .await
        .unwrap();
    let out = schedules_tick(&ctx).await.unwrap();
    assert_eq!(out.ran_schedule_ids.len(), 1);

    // The materialised invoice should now be Issued.
    let inv = InvoiceRepo::new(&pool)
        .get(&out.drafts[0].invoice.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inv.state, InvoiceState::Issued);
    assert!(inv.number.is_some());
}

#[tokio::test]
async fn schedules_tick_stamps_invoice_schedule_id_provenance() {
    // T-0039: invoices materialised by schedules_tick carry the
    // originating schedule's id on `invoice.schedule_id`. Direct drafts
    // (via draft_invoice) keep it None. The bus event payload is
    // unchanged — it also still carries schedule_id.
    let (mut ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    ctx = ctx.with_clock(Arc::new(FrozenClock(
        Utc.with_ymd_and_hms(2026, 6, 5, 0, 0, 0).unwrap(),
    )));

    // Direct draft (manual): schedule_id must be None.
    let direct = draft_invoice(&ctx, draft_input(&cust, None))
        .await
        .expect("direct draft ok");
    assert!(
        direct.invoice.schedule_id.is_none(),
        "direct draft must NOT carry schedule_id",
    );
    let direct_persisted = InvoiceRepo::new(&pool)
        .get(&direct.invoice.id)
        .await
        .unwrap()
        .unwrap();
    assert!(direct_persisted.schedule_id.is_none());

    // Materialised draft: schedule_id == originating schedule's id.
    let created = schedule_create(&ctx, schedule_input(&cust, false))
        .await
        .unwrap();
    let out = schedules_tick(&ctx).await.expect("tick ok");
    assert_eq!(out.ran_schedule_ids.len(), 1);
    assert_eq!(
        out.drafts[0].invoice.schedule_id.as_ref(),
        Some(&created.schedule.id),
        "materialised invoice in command output must carry schedule_id",
    );

    // Verify the column was actually persisted (not just the in-memory
    // struct populated). This is the durability assertion.
    let materialised = InvoiceRepo::new(&pool)
        .get(&out.drafts[0].invoice.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        materialised.schedule_id.as_ref(),
        Some(&created.schedule.id),
        "persisted invoice row must carry schedule_id",
    );
}

// ---------------------------------------------------------------------
// reminders (T-0013)
// ---------------------------------------------------------------------

#[tokio::test]
async fn reminder_schedule_then_cancel() {
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();

    let scheduled_at = ctx.clock.now() + chrono::Duration::days(7);
    let out = reminder_schedule(
        &ctx,
        ReminderScheduleInput {
            invoice_id: drafted.invoice.id.clone(),
            scheduled_at,
            channel_scheme: ReminderChannel::Webhook,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("schedule ok");

    assert_eq!(out.reminder.state, ReminderState::Scheduled);
    assert_eq!(out.reminder.channel, ReminderChannel::Webhook);

    let cancelled = reminder_cancel(
        &ctx,
        ReminderCancelInput {
            reminder_id: out.reminder.id.clone(),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap();
    assert_eq!(cancelled.reminder.state, ReminderState::Cancelled);
}

#[tokio::test]
async fn reminder_schedule_rejects_past_datetime() {
    let (ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();
    let err = reminder_schedule(
        &ctx,
        ReminderScheduleInput {
            invoice_id: drafted.invoice.id.clone(),
            scheduled_at: ctx.clock.now() - chrono::Duration::days(1),
            channel_scheme: ReminderChannel::Bus,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, CoreError::Validation(_)));
}

#[tokio::test]
async fn reminders_tick_dispatches_due_reminders() {
    let (mut ctx, pool, _publisher, _blob_dir) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();

    // Schedule a reminder in the future.
    let scheduled_at = ctx.clock.now() + chrono::Duration::hours(1);
    let scheduled = reminder_schedule(
        &ctx,
        ReminderScheduleInput {
            invoice_id: drafted.invoice.id.clone(),
            scheduled_at,
            channel_scheme: ReminderChannel::Stdout,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap();

    // Now advance the clock past scheduled_at.
    ctx = ctx.with_clock(Arc::new(FrozenClock(
        scheduled_at + chrono::Duration::minutes(5),
    )));

    let out = reminders_tick(&ctx).await.unwrap();
    assert_eq!(out.sent_reminders.len(), 1);
    assert_eq!(out.sent_reminders[0].id, scheduled.reminder.id);
    assert!(out.sent_reminders[0].sent_at.is_some());

    // Second tick: nothing to do.
    let out2 = reminders_tick(&ctx).await.unwrap();
    assert!(out2.sent_reminders.is_empty());
}
