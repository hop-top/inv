//! Integration tests for inv-store against an in-memory sqlite pool.
//!
//! Each `#[tokio::test]` opens its own pool (sqlite `:memory:` is
//! per-connection, but the pool's single connection variant means we
//! get a stable, isolated db per test).

#![cfg(feature = "sqlite")]

use std::collections::BTreeMap;
use std::str::FromStr;

use chrono::{NaiveDate, TimeZone, Utc};
use rust_decimal::Decimal;

use inv_core::domain::address::Address;
use inv_core::domain::creditnote::{CreditNote, CreditNoteState};
use inv_core::domain::customer::Customer;
use inv_core::domain::ids::{
    CreditNoteId, CustomerId, HistoryId, InvoiceId, LineId, ReminderId, ScheduleId,
};
use inv_core::domain::invoice::{
    HistoryChannel, Invoice, InvoiceLine, InvoiceState, InvoiceStateHistory, TaxCategory,
};
use inv_core::domain::jurisdiction::Jurisdiction;
use inv_core::domain::money::Currency;
use inv_core::domain::reminder::{Reminder, ReminderChannel, ReminderState};
use inv_core::domain::schedule::{Cadence, Schedule, ScheduleLine, ScheduleState};

use inv_store::pool::{backend_of, connect, Backend};
use inv_store::repo::bus_inbox::BusInboxRecord;
use inv_store::repo::invoice::InvoiceFilter;
use inv_store::repo::{
    BusInboxRepo, CreditNoteRepo, CustomerRepo, InvoiceHistoryRepo, InvoiceLineRepo, InvoiceRepo,
    ReminderRepo, ScheduleRepo,
};
use inv_store::run_migrations;

/// Open an in-memory sqlite pool, apply migrations.
async fn fresh_pool() -> inv_store::pool::Pool {
    let pool = connect("sqlite::memory:").await.expect("connect");
    assert_eq!(backend_of(&pool).await.unwrap(), Backend::Sqlite);
    run_migrations(&pool).await.expect("migrate");
    pool
}

fn sample_customer() -> Customer {
    Customer {
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
        created_at: Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap(),
    }
}

fn sample_invoice(customer_id: &CustomerId) -> Invoice {
    Invoice {
        id: InvoiceId::new(),
        number: None,
        customer_id: customer_id.clone(),
        seller_jurisdiction: Jurisdiction::QuebecCa,
        currency: Currency::CAD,
        state: InvoiceState::Draft,
        issued_at: None,
        due_at: None,
        sent_at: None,
        viewed_at: None,
        paid_at: None,
        voided_at: None,
        subtotal: Decimal::from_str("1250.00").unwrap(),
        tax_total: Decimal::from_str("187.19").unwrap(),
        total: Decimal::from_str("1437.19").unwrap(),
        amount_paid: Decimal::ZERO,
        schedule_id: None,
        template_path: None,
        pdf_blob_ref: None,
        idempotency_key: None,
        nexus_review: false,
        metadata: BTreeMap::new(),
        created_at: Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap(),
    }
}

#[tokio::test]
async fn migrations_run_cleanly() {
    let _pool = fresh_pool().await;
    // Re-applying should be a no-op (sqlx migrator tracks _sqlx_migrations).
    let pool = fresh_pool().await;
    run_migrations(&pool).await.expect("re-apply");
}

#[tokio::test]
async fn customer_round_trip() {
    let pool = fresh_pool().await;
    let repo = CustomerRepo::new(&pool);
    let c = sample_customer();
    repo.save(&c).await.expect("save");

    let back = repo.get(&c.id).await.expect("get").expect("present");
    assert_eq!(back, c);
}

#[tokio::test]
async fn invoice_and_line_round_trip() {
    let pool = fresh_pool().await;
    let cust_repo = CustomerRepo::new(&pool);
    let inv_repo = InvoiceRepo::new(&pool);
    let line_repo = InvoiceLineRepo::new(&pool);

    let c = sample_customer();
    cust_repo.save(&c).await.unwrap();
    let inv = sample_invoice(&c.id);
    inv_repo.save(&inv).await.unwrap();

    let line = InvoiceLine {
        id: LineId::new(),
        invoice_id: inv.id.clone(),
        position: 0,
        description: "Consulting".into(),
        quantity: Decimal::from_str("10").unwrap(),
        unit_price: Decimal::from_str("125.00").unwrap(),
        tax_rate_ids: vec!["ca-qc-gst".into(), "ca-qc-qst".into()],
        tax_category: TaxCategory::Standard,
        tax_amount: Decimal::from_str("187.19").unwrap(),
        line_total: Decimal::from_str("1437.19").unwrap(),
        metadata: BTreeMap::new(),
    };
    line_repo.save(&line).await.unwrap();

    let inv_back = inv_repo.get(&inv.id).await.unwrap().unwrap();
    assert_eq!(inv_back, inv);

    let lines = line_repo.list_for_invoice(&inv.id).await.unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], line);
}

#[tokio::test]
async fn decimal_text_round_trip_preserves_precision() {
    // Per design §5, sqlite Decimal columns are TEXT storing the
    // canonical `rust_decimal::Decimal` string.
    let pool = fresh_pool().await;
    let cust_repo = CustomerRepo::new(&pool);
    let inv_repo = InvoiceRepo::new(&pool);

    let c = sample_customer();
    cust_repo.save(&c).await.unwrap();

    let mut inv = sample_invoice(&c.id);
    inv.subtotal = Decimal::from_str("123.45").unwrap();
    inv.tax_total = Decimal::from_str("0.00").unwrap();
    inv.total = Decimal::from_str("123.45").unwrap();
    inv.amount_paid = Decimal::from_str("0.123456789").unwrap();
    inv_repo.save(&inv).await.unwrap();

    let back = inv_repo.get(&inv.id).await.unwrap().unwrap();
    assert_eq!(back.subtotal, Decimal::from_str("123.45").unwrap());
    assert_eq!(back.amount_paid, Decimal::from_str("0.123456789").unwrap());
    assert_eq!(back, inv);
}

#[tokio::test]
async fn invoice_list_filters_by_state_and_customer() {
    let pool = fresh_pool().await;
    let cust_repo = CustomerRepo::new(&pool);
    let inv_repo = InvoiceRepo::new(&pool);

    let c1 = sample_customer();
    cust_repo.save(&c1).await.unwrap();

    let mut draft = sample_invoice(&c1.id);
    let mut issued = sample_invoice(&c1.id);
    issued.state = InvoiceState::Issued;
    issued.number = Some("INV-2026-0001".into());
    issued.issued_at = Some(Utc::now());
    inv_repo.save(&draft).await.unwrap();
    inv_repo.save(&issued).await.unwrap();

    let drafts = inv_repo
        .list(&InvoiceFilter {
            state: Some(InvoiceState::Draft),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].state, InvoiceState::Draft);

    let by_cust = inv_repo
        .list(&InvoiceFilter {
            customer_id: Some(c1.id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(by_cust.len(), 2);

    // Touch the variables we mutated above so clippy doesn't grumble.
    draft.state = InvoiceState::Draft;
    assert_eq!(draft.state, InvoiceState::Draft);
    assert!(issued.number.is_some());
}

#[tokio::test]
async fn credit_note_round_trip() {
    let pool = fresh_pool().await;
    let cust_repo = CustomerRepo::new(&pool);
    let inv_repo = InvoiceRepo::new(&pool);
    let cn_repo = CreditNoteRepo::new(&pool);

    let c = sample_customer();
    cust_repo.save(&c).await.unwrap();
    let inv = sample_invoice(&c.id);
    inv_repo.save(&inv).await.unwrap();

    let cn = CreditNote {
        id: CreditNoteId::new(),
        number: Some("CN-2026-0001".into()),
        invoice_id: inv.id.clone(),
        state: CreditNoteState::Issued,
        amount: Decimal::from_str("250.00").unwrap(),
        currency: Currency::CAD,
        reason: Some("duplicate".into()),
        refund_ref: None,
        issued_at: Some(Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap()),
        created_at: Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).unwrap(),
        metadata: BTreeMap::new(),
    };
    cn_repo.save(&cn).await.unwrap();

    let back = cn_repo.get(&cn.id).await.unwrap().unwrap();
    assert_eq!(back, cn);

    let list = cn_repo.list_for_invoice(&inv.id).await.unwrap();
    assert_eq!(list.len(), 1);
}

#[tokio::test]
async fn schedule_round_trip() {
    let pool = fresh_pool().await;
    let cust_repo = CustomerRepo::new(&pool);
    let sched_repo = ScheduleRepo::new(&pool);

    let c = sample_customer();
    cust_repo.save(&c).await.unwrap();

    let sched = Schedule {
        id: ScheduleId::new(),
        customer_id: c.id.clone(),
        template_lines: vec![ScheduleLine {
            description: "Monthly retainer".into(),
            quantity: Decimal::from_str("1").unwrap(),
            unit_price: Decimal::from_str("2500.00").unwrap(),
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
        state: ScheduleState::Active,
        metadata: BTreeMap::new(),
        created_at: Utc.with_ymd_and_hms(2026, 1, 31, 0, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2026, 1, 31, 0, 0, 0).unwrap(),
    };
    sched_repo.save(&sched).await.unwrap();
    let back = sched_repo.get(&sched.id).await.unwrap().unwrap();
    assert_eq!(back, sched);

    let due = sched_repo
        .due(NaiveDate::from_ymd_opt(2026, 2, 1).unwrap())
        .await
        .unwrap();
    assert_eq!(due.len(), 1);
}

#[tokio::test]
async fn reminder_round_trip() {
    let pool = fresh_pool().await;
    let cust_repo = CustomerRepo::new(&pool);
    let inv_repo = InvoiceRepo::new(&pool);
    let rem_repo = ReminderRepo::new(&pool);

    let c = sample_customer();
    cust_repo.save(&c).await.unwrap();
    let inv = sample_invoice(&c.id);
    inv_repo.save(&inv).await.unwrap();

    let rem = Reminder {
        id: ReminderId::new(),
        invoice_id: inv.id.clone(),
        scheduled_at: Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap(),
        sent_at: None,
        channel: ReminderChannel::Webhook,
        state: ReminderState::Scheduled,
    };
    rem_repo.save(&rem).await.unwrap();
    let back = rem_repo.get(&rem.id).await.unwrap().unwrap();
    assert_eq!(back, rem);

    let due = rem_repo
        .due(Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap())
        .await
        .unwrap();
    assert_eq!(due.len(), 1);
}

#[tokio::test]
async fn invoice_history_records_and_outbox() {
    let pool = fresh_pool().await;
    let cust_repo = CustomerRepo::new(&pool);
    let inv_repo = InvoiceRepo::new(&pool);
    let hist_repo = InvoiceHistoryRepo::new(&pool);

    let c = sample_customer();
    cust_repo.save(&c).await.unwrap();
    let inv = sample_invoice(&c.id);
    inv_repo.save(&inv).await.unwrap();

    let h = InvoiceStateHistory {
        id: HistoryId::new(),
        invoice_id: inv.id.clone(),
        from_state: None,
        to_state: InvoiceState::Draft,
        event: "draft".into(),
        actor: Some("jad".into()),
        channel: HistoryChannel::Cli,
        bus_event_id: None,
        reason: None,
        occurred_at: Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap(),
        published_at: None,
        metadata: BTreeMap::new(),
    };
    hist_repo.save(&h).await.unwrap();

    let rows = hist_repo.list_for_invoice(&inv.id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0], h);

    // Outbox: published_at IS NULL → row is pending.
    let pending = hist_repo.pending_outbox(10).await.unwrap();
    assert_eq!(pending.len(), 1);

    hist_repo.mark_published(&h.id).await.unwrap();
    let pending = hist_repo.pending_outbox(10).await.unwrap();
    assert!(pending.is_empty());
}

#[tokio::test]
async fn bus_inbox_dedups_on_event_id() {
    let pool = fresh_pool().await;
    let inbox = BusInboxRepo::new(&pool);

    let rec = BusInboxRecord {
        event_id: "evt-abc-123".into(),
        topic: "fin.billing.payment.received".into(),
        source: "fin".into(),
        received_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        payload_json: r#"{"amount":"100.00"}"#.into(),
        processed_at: None,
        invoice_id: None,
    };
    assert!(inbox.try_insert(&rec).await.unwrap());
    // Second insert with same event_id returns false (dedup).
    assert!(!inbox.try_insert(&rec).await.unwrap());

    let back = inbox.get(&rec.event_id).await.unwrap().unwrap();
    assert_eq!(back, rec);

    inbox.mark_processed(&rec.event_id).await.unwrap();
    let back2 = inbox.get(&rec.event_id).await.unwrap().unwrap();
    assert!(back2.processed_at.is_some());
}

// ---------------------------------------------------------------------
// Regression: T-0024 — InvoiceRepo::save() must NOT trigger
// ON DELETE CASCADE on invoice_state_history.
// ---------------------------------------------------------------------

#[tokio::test]
async fn invoice_save_preserves_history_rows() {
    let pool = fresh_pool().await;
    CustomerRepo::new(&pool).save(&sample_customer()).await.unwrap();

    let cust = sample_customer();
    CustomerRepo::new(&pool).save(&cust).await.unwrap();
    let inv = sample_invoice(&cust.id);
    let inv_repo = InvoiceRepo::new(&pool);
    let hist_repo = InvoiceHistoryRepo::new(&pool);

    inv_repo.save(&inv).await.unwrap();

    // Write a history row, then save the invoice again (simulating an
    // FSM transition). With the old DELETE+INSERT pattern, the ON DELETE
    // CASCADE on invoice_state_history would wipe the history row.
    let h = InvoiceStateHistory {
        id: HistoryId::new(),
        invoice_id: inv.id.clone(),
        from_state: None,
        to_state: InvoiceState::Draft,
        event: "draft".into(),
        actor: Some("jad".into()),
        channel: HistoryChannel::Cli,
        bus_event_id: None,
        reason: None,
        occurred_at: Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, 0).unwrap(),
        published_at: None,
        metadata: BTreeMap::new(),
    };
    hist_repo.save(&h).await.unwrap();

    let mut inv_updated = inv.clone();
    inv_updated.state = InvoiceState::Issued;
    inv_updated.number = Some("INV-2026-0001".into());
    inv_updated.issued_at =
        Some(Utc.with_ymd_and_hms(2026, 1, 3, 10, 0, 0).unwrap());
    inv_repo.save(&inv_updated).await.unwrap();

    // The history row must still be there after the second save.
    let rows = hist_repo.list_for_invoice(&inv.id).await.unwrap();
    assert_eq!(rows.len(), 1, "history row was wiped by repeated InvoiceRepo::save() — CASCADE regression");
    assert_eq!(rows[0], h);

    // The invoice itself reflects the updated state.
    let back = inv_repo.get(&inv.id).await.unwrap().unwrap();
    assert_eq!(back.state, InvoiceState::Issued);
    assert_eq!(back.number.as_deref(), Some("INV-2026-0001"));
    assert!(back.issued_at.is_some());
    // created_at is preserved across upserts.
    assert_eq!(back.created_at, inv.created_at);
}

#[tokio::test]
async fn customer_save_does_not_fk_violate_when_referenced() {
    let pool = fresh_pool().await;
    let repo = CustomerRepo::new(&pool);

    let mut c = sample_customer();
    repo.save(&c).await.unwrap();

    // Insert a referencing invoice — with the old DELETE+INSERT pattern,
    // a second save of the customer would have FK-violated.
    let inv = sample_invoice(&c.id);
    InvoiceRepo::new(&pool).save(&inv).await.unwrap();

    // Now save the customer again. Must succeed.
    c.display_name = "Acme Corp (renamed)".into();
    c.updated_at = Utc.with_ymd_and_hms(2026, 1, 5, 0, 0, 0).unwrap();
    repo.save(&c).await.expect("save must succeed when customer is FK-referenced");

    let back = repo.get(&c.id).await.unwrap().unwrap();
    assert_eq!(back.display_name, "Acme Corp (renamed)");
}
