//! Integration tests for `inv-bus`.
//!
//! Covers:
//! - [`LoggingPublisher`] / [`InMemoryPublisher`] basics.
//! - [`run_outbox_relay`] end-to-end against an in-memory sqlite pool
//!   seeded by `draft_invoice`.
//! - [`dispatch_inbound_event`] dedup behaviour.
//! - [`remap_topic`] config-driven canonicalisation.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::json;

use inv_bus::{
    dispatch_inbound_event, remap_topic, run_outbox_relay, InMemoryPublisher, LoggingPublisher,
    Publisher, TopicMap, TOPIC_INVOICE_DRAFTED,
};
use inv_commands::{
    draft_invoice, Actor, Channel, Clock, CoreCtx, DraftInvoiceInput, DraftLineInput,
};
use inv_core::domain::address::Address;
use inv_core::domain::customer::Customer;
use inv_core::domain::ids::CustomerId;
use inv_core::domain::invoice::TaxCategory;
use inv_core::domain::jurisdiction::Jurisdiction;
use inv_core::domain::money::Currency;
use inv_core::tax::TaxTable;
use inv_store::pool::{connect, Pool};
use inv_store::repo::bus_inbox::BusInboxRepo;
use inv_store::repo::{CustomerRepo, InvoiceHistoryRepo};
use inv_store::run_migrations;

// =============================================================================
// Test scaffolding
// =============================================================================

/// Frozen-clock impl for deterministic tests.
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
"#;

fn frozen_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap()
}

async fn fresh_ctx() -> (CoreCtx, Pool) {
    let pool = connect("sqlite::memory:").await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    let (table, nexus) = TaxTable::load_from_str(FIXTURE_TOML).expect("tax fixture");
    let ctx = CoreCtx::new(pool.clone(), table, nexus)
        .with_clock(Arc::new(FrozenClock(frozen_now())));
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

fn draft_input(customer_id: &CustomerId) -> DraftInvoiceInput {
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
        idempotency_key: None,
        actor: Actor::Cli { name: "jad".into() },
        channel: Channel::Cli,
        due_at: None,
        template_path: None,
    }
}

// =============================================================================
// Publisher impls
// =============================================================================

#[tokio::test]
async fn logging_publisher_does_not_panic() {
    let p = LoggingPublisher::new();
    p.publish("inv.billing.invoice.drafted", json!({"x": 1}), Utc::now())
        .await
        .unwrap();
    p.publish("inv.billing.invoice.issued", json!({"x": 2}), Utc::now())
        .await
        .unwrap();
    p.publish("inv.billing.invoice.paid", json!({"x": 3}), Utc::now())
        .await
        .unwrap();
}

#[tokio::test]
async fn in_memory_publisher_captures_topics_and_payloads() {
    let p = InMemoryPublisher::new();
    let t = Utc::now();
    p.publish("inv.billing.invoice.drafted", json!({"id": "a"}), t)
        .await
        .unwrap();
    p.publish("inv.billing.invoice.issued", json!({"id": "b"}), t)
        .await
        .unwrap();
    p.publish("inv.billing.invoice.paid", json!({"id": "c"}), t)
        .await
        .unwrap();

    let captured = p.captured();
    assert_eq!(captured.len(), 3);
    assert_eq!(
        p.topics(),
        vec![
            "inv.billing.invoice.drafted".to_string(),
            "inv.billing.invoice.issued".to_string(),
            "inv.billing.invoice.paid".to_string(),
        ]
    );
    assert_eq!(captured[0].payload, json!({"id": "a"}));
    assert_eq!(captured[1].payload, json!({"id": "b"}));
    assert_eq!(captured[2].payload, json!({"id": "c"}));
}

// =============================================================================
// Outbox relay
// =============================================================================

#[tokio::test]
async fn outbox_relay_publishes_pending_draft_row_then_marks_published() {
    let (ctx, pool) = fresh_ctx().await;
    let customer_id = seed_customer(&pool).await;

    // 1. draft_invoice writes one history row (the initial "draft" row).
    let drafted = draft_invoice(&ctx, draft_input(&customer_id)).await.unwrap();
    assert!(!drafted.idempotency_replay);

    // The history repo should report one pending outbox row.
    let hist_repo = InvoiceHistoryRepo::new(&pool);
    let pending_before = hist_repo.pending_outbox(10).await.unwrap();
    assert_eq!(pending_before.len(), 1);
    assert!(pending_before[0].published_at.is_none());

    // 2. Run the relay against an in-memory publisher.
    let publisher = InMemoryPublisher::new();
    let stats = run_outbox_relay(&pool, &publisher, 10).await.unwrap();

    // Per the history-row → event-set mapper: initial "draft" rows
    // (from_state IS NULL) emit only the domain event — no mechanic
    // triplet, because the FSM didn't transition (object was created
    // in its initial state). See outbox.rs docs.
    assert_eq!(stats.rows_processed, 1);
    assert_eq!(stats.events_published, 1);
    let topics = publisher.topics();
    assert_eq!(topics, vec![TOPIC_INVOICE_DRAFTED.to_string()]);

    // 3. Row should now be marked published.
    let pending_after = hist_repo.pending_outbox(10).await.unwrap();
    assert!(pending_after.is_empty());

    // 4. Second run is a no-op.
    let publisher2 = InMemoryPublisher::new();
    let stats2 = run_outbox_relay(&pool, &publisher2, 10).await.unwrap();
    assert_eq!(stats2.rows_processed, 0);
    assert_eq!(stats2.events_published, 0);
    assert!(publisher2.is_empty());
}

// =============================================================================
// Inbox dedup
// =============================================================================

#[tokio::test]
async fn inbox_dispatch_returns_true_then_false_on_replay() {
    let (_ctx, pool) = fresh_ctx().await;
    let inbox = BusInboxRepo::new(&pool);

    let first = dispatch_inbound_event(
        &inbox,
        "evt-1",
        "fin.billing.charge.created",
        "fin",
        r#"{"charge_id": "ch_1"}"#,
    )
    .await
    .unwrap();
    assert!(first, "first dispatch must accept the event");

    let second = dispatch_inbound_event(
        &inbox,
        "evt-1",
        "fin.billing.charge.created",
        "fin",
        r#"{"charge_id": "ch_1"}"#,
    )
    .await
    .unwrap();
    assert!(!second, "second dispatch must dedup");
}

// =============================================================================
// Topic remap
// =============================================================================

#[test]
fn topic_remap_returns_canonical_when_configured() {
    let map = TopicMap::from_pairs([(
        "fin.finance.charge.created",
        "fin.billing.charge.created",
    )]);
    assert_eq!(
        remap_topic(&map, "fin.finance.charge.created"),
        "fin.billing.charge.created"
    );
}

#[test]
fn topic_remap_passes_through_unmapped() {
    let map = TopicMap::from_pairs([(
        "fin.finance.charge.created",
        "fin.billing.charge.created",
    )]);
    assert_eq!(
        remap_topic(&map, "fin.billing.payment.received"),
        "fin.billing.payment.received"
    );
}

// =============================================================================
// Consumer (T-0015)
// =============================================================================

use inv_bus::{Consumer, DispatchError, DispatchOutput};
use inv_commands::{issue_invoice, IssueInvoiceInput};
use inv_core::domain::invoice::InvoiceState;

#[tokio::test]
async fn consumer_charge_created_creates_draft() {
    let (ctx, pool) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let consumer = Consumer::new();

    let payload = json!({
        "customer_id": cust.to_string(),
        "currency": "CAD",
        "seller_jurisdiction": "CA-QC",
        "lines": [
            {
                "description": "Consulting",
                "quantity": "10",
                "unit_price": "125.00"
            }
        ]
    })
    .to_string();

    let out = consumer
        .dispatch(&ctx, "fin.billing.charge.created", "evt-1", &payload)
        .await
        .expect("dispatch ok");

    match out {
        DispatchOutput::Drafted(o) => {
            assert_eq!(o.invoice.state, InvoiceState::Draft);
            assert_eq!(o.invoice.customer_id, cust);
            assert_eq!(o.invoice.currency, Currency::CAD);
            assert_eq!(o.lines.len(), 1);
        }
        other => panic!("expected Drafted, got {other:?}"),
    }
}

#[tokio::test]
async fn consumer_payment_received_marks_paid() {
    let (ctx, pool) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust)).await.unwrap();
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

    let consumer = Consumer::new();
    let payload = json!({
        "invoice_ref": issued.invoice.id.to_string(),
        "amount": issued.invoice.total.to_string(),
        "received_at": frozen_now().to_rfc3339(),
    })
    .to_string();

    let out = consumer
        .dispatch(&ctx, "fin.billing.payment.received", "evt-2", &payload)
        .await
        .expect("dispatch ok");

    match out {
        DispatchOutput::Paid(o) => {
            assert!(o.fully_paid);
            assert_eq!(o.invoice.state, InvoiceState::Paid);
        }
        other => panic!("expected Paid, got {other:?}"),
    }
}

#[tokio::test]
async fn consumer_payment_received_accepts_uri_form() {
    // invoice_ref as `inv://invoice/<typeid>` should resolve identically.
    let (ctx, pool) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust)).await.unwrap();
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

    let consumer = Consumer::new();
    let payload = json!({
        "invoice_ref": format!("inv://invoice/{}", issued.invoice.id),
        "amount": issued.invoice.total.to_string(),
        "received_at": frozen_now().to_rfc3339(),
    })
    .to_string();

    let out = consumer
        .dispatch(&ctx, "fin.billing.payment.received", "evt-uri", &payload)
        .await
        .expect("dispatch ok");
    assert!(matches!(out, DispatchOutput::Paid(_)));
}

#[tokio::test]
async fn consumer_payment_refunded_creates_credit_note() {
    let (ctx, pool) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let drafted = draft_invoice(&ctx, draft_input(&cust)).await.unwrap();
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

    let consumer = Consumer::new();
    let payload = json!({
        "invoice_ref": issued.invoice.id.to_string(),
        "amount": "50.00",
        "reason": "duplicate charge",
        "refund_id": "stripe-refund-abc"
    })
    .to_string();

    let out = consumer
        .dispatch(&ctx, "fin.billing.payment.refunded", "evt-3", &payload)
        .await
        .expect("dispatch ok");

    match out {
        DispatchOutput::Refunded(o) => {
            assert_eq!(o.credit_note.amount, Decimal::from_str("50.00").unwrap());
            assert_eq!(o.credit_note.refund_ref.as_deref(), Some("stripe-refund-abc"));
            assert_eq!(o.credit_note.invoice_id, issued.invoice.id);
        }
        other => panic!("expected Refunded, got {other:?}"),
    }
}

#[tokio::test]
async fn consumer_unknown_topic_rejected() {
    let (ctx, _pool) = fresh_ctx().await;
    let consumer = Consumer::new();
    let err = consumer
        .dispatch(&ctx, "fin.unknown.thing.happened", "evt-x", "{}")
        .await
        .unwrap_err();
    assert!(matches!(err, DispatchError::UnknownTopic(_)));
}

#[tokio::test]
async fn consumer_invalid_payload_rejected() {
    let (ctx, _pool) = fresh_ctx().await;
    let consumer = Consumer::new();
    let err = consumer
        .dispatch(
            &ctx,
            "fin.billing.charge.created",
            "evt-y",
            r#"{"customer_id": "not-a-typeid", "currency": "CAD", "lines": []}"#,
        )
        .await
        .unwrap_err();
    // First failure point is customer_id parse — InvalidPayload, not Decode.
    match err {
        DispatchError::InvalidPayload { topic, .. } => {
            assert_eq!(topic, "fin.billing.charge.created");
        }
        DispatchError::Command { .. } => {
            // also acceptable — empty lines would have made it past parse and
            // failed validation
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[tokio::test]
async fn consumer_applies_topic_remap() {
    let (ctx, pool) = fresh_ctx().await;
    let cust = seed_customer(&pool).await;
    let map = TopicMap::from_pairs([(
        "fin.finance.charge.created",
        "fin.billing.charge.created",
    )]);
    let consumer = Consumer::with_remap(map);
    let payload = json!({
        "customer_id": cust.to_string(),
        "currency": "CAD",
        "lines": [{"description": "X", "quantity": "1", "unit_price": "100"}]
    })
    .to_string();
    let out = consumer
        .dispatch(&ctx, "fin.finance.charge.created", "evt-remap", &payload)
        .await
        .expect("dispatch via remap");
    assert!(matches!(out, DispatchOutput::Drafted(_)));
}
