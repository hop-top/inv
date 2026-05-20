//! Integration tests for the inv-commands one-command-core layer.
//!
//! Each test opens an in-memory sqlite pool, applies the inv-store
//! migrations, seeds a customer, and exercises one or more of
//! `draft_invoice` / `issue_invoice` / `send_invoice`. The dev-dep
//! `inv-store` is configured with the `sqlite` feature, so the in-memory
//! pool always works without a cfg guard.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;

use inv_core::domain::address::Address;
use inv_core::domain::customer::Customer;
use inv_core::domain::ids::CustomerId;
use inv_core::domain::invoice::{InvoiceState, TaxCategory};
use inv_core::domain::jurisdiction::Jurisdiction;
use inv_core::domain::money::Currency;
use inv_core::state::TransitionError;
use inv_core::tax::TaxTable;

use inv_commands::{
    create_credit_note, draft_invoice, issue_credit_note, issue_invoice, mark_overdue_ticker,
    mark_paid, send_invoice, void_invoice, Actor, Channel, Clock, CoreCtx, CoreError,
    CreateCreditNoteInput, DraftInvoiceInput, DraftLineInput, IssueCreditNoteInput,
    IssueInvoiceInput, MarkPaidInput, SendInvoiceInput, VoidInvoiceInput,
};
use inv_core::domain::creditnote::CreditNoteState;
use inv_store::pool::{connect, Pool};
use inv_store::repo::history::InvoiceHistoryRepo;
use inv_store::repo::{CreditNoteRepo, CustomerRepo, InvoiceRepo};
use inv_store::run_migrations;

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

async fn fresh_ctx() -> (CoreCtx, Pool) {
    let pool = connect("sqlite::memory:").await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    let (table, nexus) = TaxTable::load_from_str(FIXTURE_TOML).expect("tax fixture");
    let ctx = CoreCtx::new(pool.clone(), table, nexus).with_clock(Arc::new(FrozenClock(frozen_now())));
    (ctx, pool)
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

fn draft_input(
    customer_id: &CustomerId,
    idempotency: Option<&str>,
) -> DraftInvoiceInput {
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
        actor: Actor::Cli {
            name: "jad".into(),
        },
        channel: Channel::Cli,
        due_at: None,
        template_path: None,
    }
}

// ---------------------------------------------------------------------
// draft_invoice
// ---------------------------------------------------------------------

#[tokio::test]
async fn draft_happy_path_creates_draft_with_computed_totals() {
    let (ctx, pool) = fresh_ctx().await;
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

    // emitted: inv.billing.invoice.drafted.
    assert_eq!(out.emitted_events.len(), 1);
    assert_eq!(out.emitted_events[0].topic, "inv.billing.invoice.drafted");

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
    let (ctx, pool) = fresh_ctx().await;
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
    assert!(second.emitted_events.is_empty(), "replay must not emit");

    // Still only one history row.
    let hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&first.invoice.id)
        .await
        .unwrap();
    assert_eq!(hist.len(), 1);
}

#[tokio::test]
async fn draft_validation_rejects_empty_lines() {
    let (ctx, pool) = fresh_ctx().await;
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
    let (ctx, pool) = fresh_ctx().await;
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
    let (ctx, pool) = fresh_ctx().await;
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
    let (ctx, pool) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None))
        .await
        .unwrap();

    let out = issue_invoice(
        &ctx,
        IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli {
                name: "jad".into(),
            },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("issue ok");

    assert_eq!(out.invoice.state, InvoiceState::Issued);
    assert_eq!(out.invoice.number.as_deref(), Some("INV-2026-0001"));
    assert!(out.invoice.issued_at.is_some());
    // GST 5% + QST 9.975% on 1250 = 187.1875 → banker's-round to 187.19.
    assert_eq!(
        out.invoice.tax_total,
        Decimal::from_str("187.19").unwrap()
    );
    assert_eq!(out.invoice.total, Decimal::from_str("1437.19").unwrap());
    assert_eq!(out.lines.len(), 1);
    assert_eq!(out.lines[0].tax_rate_ids.len(), 2);
    assert!(!out.html.is_empty());
    assert!(!out.pdf.is_empty());

    // Mechanic-triplet + domain `.issued`.
    let topics: Vec<&str> = out
        .emitted_events
        .iter()
        .map(|e| e.topic.as_str())
        .collect();
    assert!(topics.contains(&"inv.billing.invoice.proposed"));
    assert!(topics.contains(&"inv.billing.invoice.transitioned"));
    assert!(topics.contains(&"inv.billing.invoice.entered"));
    assert!(topics.contains(&"inv.billing.invoice.issued"));

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
    let (ctx, pool) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None))
        .await
        .unwrap();
    issue_invoice(
        &ctx,
        IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli {
                name: "jad".into(),
            },
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
            actor: Actor::Cli {
                name: "jad".into(),
            },
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

// ---------------------------------------------------------------------
// send_invoice
// ---------------------------------------------------------------------

#[tokio::test]
async fn send_file_writes_bytes_and_transitions_to_sent() {
    let (ctx, pool) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None))
        .await
        .unwrap();
    let issued = issue_invoice(
        &ctx,
        IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli {
                name: "jad".into(),
            },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap();

    let dir = std::env::temp_dir().join(format!(
        "inv-commands-test-{}-{}",
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
            actor: Actor::Cli {
                name: "jad".into(),
            },
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

    // Topics fired.
    let topics: Vec<&str> = out
        .emitted_events
        .iter()
        .map(|e| e.topic.as_str())
        .collect();
    assert!(topics.contains(&"inv.billing.invoice.sent"));
    assert!(topics.contains(&"inv.billing.invoice.transitioned"));

    // history row for "send".
    let hist = InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&out.invoice.id)
        .await
        .unwrap();
    assert!(hist.iter().any(|h| h.event == "send"));
}

#[tokio::test]
async fn send_stdout_writes_to_injected_sink() {
    let (ctx, pool) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None))
        .await
        .unwrap();
    let issued = issue_invoice(
        &ctx,
        IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli {
                name: "jad".into(),
            },
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
            actor: Actor::Cli {
                name: "jad".into(),
            },
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
    let (ctx, pool) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None))
        .await
        .unwrap();
    let issued = issue_invoice(
        &ctx,
        IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli {
                name: "jad".into(),
            },
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
            actor: Actor::Cli {
                name: "jad".into(),
            },
            channel: Channel::Cli,
            sink: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, CoreError::NotImplemented(_)));
}

// ---------------------------------------------------------------------
// mark_paid (T-0012)
// ---------------------------------------------------------------------

#[tokio::test]
async fn mark_paid_full_settles_invoice() {
    let (ctx, pool) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None))
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

    let topics: Vec<&str> = out.emitted_events.iter().map(|e| e.topic.as_str()).collect();
    assert!(topics.contains(&"inv.billing.invoice.paid"));

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
    let (ctx, pool) = fresh_ctx().await;
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
    let (ctx, _pool) = fresh_ctx().await;
    let err = mark_paid(
        &ctx,
        MarkPaidInput {
            invoice_id: inv_core::domain::ids::InvoiceId::new(),
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
    let (ctx, pool) = fresh_ctx().await;
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

    let topics: Vec<&str> = out.emitted_events.iter().map(|e| e.topic.as_str()).collect();
    assert!(topics.contains(&"inv.billing.invoice.voided"));
}

#[tokio::test]
async fn void_after_payment_rejected_by_fsm() {
    let (ctx, pool) = fresh_ctx().await;
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
    let (ctx, pool) = fresh_ctx().await;
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
    assert_eq!(cn_draft.credit_note.amount, Decimal::from_str("100.00").unwrap());
    assert_eq!(cn_draft.credit_note.refund_ref.as_deref(), Some("refund-evt-1"));
    let topics: Vec<&str> = cn_draft.emitted_events.iter().map(|e| e.topic.as_str()).collect();
    assert!(topics.contains(&"inv.billing.creditnote.drafted"));

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
    assert!(cn_issued.credit_note.number.as_deref().unwrap_or("").starts_with("CN-2026-"));
    assert!(cn_issued.credit_note.issued_at.is_some());

    // Verify the cn is in the DB at the issued state.
    let back = CreditNoteRepo::new(&pool)
        .get(&cn_draft.credit_note.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back.state, CreditNoteState::Issued);

    let topics: Vec<&str> = cn_issued.emitted_events.iter().map(|e| e.topic.as_str()).collect();
    assert!(topics.contains(&"inv.billing.creditnote.proposed"));
    assert!(topics.contains(&"inv.billing.creditnote.transitioned"));
    assert!(topics.contains(&"inv.billing.creditnote.entered"));
    assert!(topics.contains(&"inv.billing.creditnote.issued"));
}

#[tokio::test]
async fn credit_note_rejects_non_positive_amount() {
    let (ctx, pool) = fresh_ctx().await;
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
    let (ctx, pool) = fresh_ctx().await;
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

    let out = mark_overdue_ticker(&ctx).await.expect("tick ok");
    assert_eq!(out.overdue_invoices.len(), 1);
    assert_eq!(out.overdue_invoices[0].id, issued.invoice.id);

    let topics: Vec<&str> = out.emitted_events.iter().map(|e| e.topic.as_str()).collect();
    assert_eq!(topics, vec!["inv.billing.invoice.overdue"]);

    // State unchanged — overdue is a flag, not an FSM state.
    let back = InvoiceRepo::new(&pool).get(&issued.invoice.id).await.unwrap().unwrap();
    assert_eq!(back.state, InvoiceState::Issued);
}

#[tokio::test]
async fn overdue_ticker_ignores_unset_due_at() {
    let (ctx, pool) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust, None)).await.unwrap();
    // Don't issue — Draft is not in the overdue-eligible set anyway.
    let _ = drafted;
    let out = mark_overdue_ticker(&ctx).await.unwrap();
    assert!(out.overdue_invoices.is_empty());
    assert!(out.emitted_events.is_empty());
}
