//! Story freelancer-qc-03: reminder ladder + overdue ticker.
//!
//! Surfaces: CLI (in-process via inv-commands), tickers (reminders +
//! overdue), store (reminders + invoice_state_history), bus events.
//!
//! No xrr — pure in-process, frozen-clock advance.

mod common;

use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use rust_decimal::Decimal;

use inv_commands::{
    draft_invoice, issue_invoice, mark_overdue_ticker, reminder_schedule, reminders_tick,
    send_invoice, Actor, Channel, DraftInvoiceInput, DraftLineInput, IssueInvoiceInput,
    ReminderScheduleInput, SendInvoiceInput,
};
use inv_core::domain::invoice::{InvoiceState, TaxCategory};
use inv_core::domain::jurisdiction::Jurisdiction;
use inv_core::domain::money::Currency;
use inv_core::domain::reminder::{ReminderChannel, ReminderState};
use inv_store::repo::invoice::InvoiceRepo;

#[tokio::test]
async fn reminder_ladder_dispatches_and_overdue_flags() {
    let (mut ctx, pool, captured, _blob) = common::fresh_ctx().await;
    let cust = common::seed_customer_qc(&pool).await;

    // Drive draft → issue → send to put the invoice in Sent state with
    // a due_at in the future relative to the ladder's first reminder.
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
            due_at: Some("2026-06-30T00:00:00Z".parse().unwrap()),
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
    .unwrap();
    assert_eq!(sent.invoice.state, InvoiceState::Sent);

    // ----- When: three reminders scheduled at increasing dates. ------
    let inv_id = sent.invoice.id.clone();
    let mk = |at: chrono::DateTime<Utc>| ReminderScheduleInput {
        invoice_id: inv_id.clone(),
        scheduled_at: at,
        // Stdout used as the v1 channel surrogate for "link" — the
        // ticker re-renders + writes to the sink rather than performing
        // SMTP / SMS / actual link mint (out of scope at v1 per the
        // story). The FSM behaviour (Sent → Sent self-edge) is the
        // assertion we care about.
        channel_scheme: ReminderChannel::Stdout,
        actor: Actor::Cli { name: "jad".into() },
        channel: Channel::Cli,
    };
    let r1 = reminder_schedule(&ctx, mk("2026-07-07T09:00:00Z".parse().unwrap()))
        .await
        .expect("r1");
    let _r2 = reminder_schedule(&ctx, mk("2026-07-14T09:00:00Z".parse().unwrap()))
        .await
        .expect("r2");
    let _r3 = reminder_schedule(&ctx, mk("2026-07-28T09:00:00Z".parse().unwrap()))
        .await
        .expect("r3");

    // ----- Then: each reminder is scheduled + event emitted. ---------
    assert_eq!(r1.reminder.state, ReminderState::Scheduled);
    let topics = captured.topics();
    assert!(topics.contains(&"inv.billing.reminder.scheduled".to_string()));

    // ----- Given today is 2026-07-07T09:01:00Z (past the first). -----
    ctx = ctx.with_clock(Arc::new(common::FrozenClock(
        Utc.with_ymd_and_hms(2026, 7, 7, 9, 1, 0).unwrap(),
    )));
    // Snapshot publisher pre-tick so we can isolate this tick's emissions.
    let topics_before_tick = captured.topics();
    let tick = reminders_tick(&ctx).await.expect("tick");

    // ----- Then: first reminder dispatched + invoice stays Sent. -----
    assert_eq!(tick.sent_reminders.len(), 1);
    assert_eq!(tick.sent_reminders[0].id, r1.reminder.id);
    let back = InvoiceRepo::new(&pool).get(&inv_id).await.unwrap().unwrap();
    assert_eq!(
        back.state,
        InvoiceState::Sent,
        "FSM stays Sent on reminder self-edge"
    );
    let topics_after_tick = captured.topics();
    let new_topics: Vec<&str> = topics_after_tick[topics_before_tick.len()..]
        .iter()
        .map(|s| s.as_str())
        .collect();
    assert!(new_topics.contains(&"inv.billing.reminder.sent"));
    // The invoice's FSM did NOT move → no transitioned/entered events
    // from this tick (a prior issue/send pair did publish those — that's
    // why we slice on the pre-tick snapshot rather than the cumulative
    // capture).
    assert!(
        !new_topics.contains(&"inv.billing.invoice.transitioned"),
        "self-edge must not emit transitioned: {new_topics:?}"
    );

    // ----- Given today is 2026-07-01 (one day past due_at=2026-06-30).
    ctx = ctx.with_clock(Arc::new(common::FrozenClock(
        Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
    )));

    // ----- When: overdue ticker fires. -------------------------------
    let overdue = mark_overdue_ticker(&ctx).await.expect("overdue tick");

    // ----- Then: invoice.overdue emitted, state still Sent. ----------
    assert_eq!(overdue.overdue_invoices.len(), 1);
    assert_eq!(overdue.overdue_invoices[0].id, inv_id);
    let topics = captured.topics();
    assert!(topics.contains(&"inv.billing.invoice.overdue".to_string()));
    let back2 = InvoiceRepo::new(&pool).get(&inv_id).await.unwrap().unwrap();
    assert_eq!(
        back2.state,
        InvoiceState::Sent,
        "overdue is a flag, not a state"
    );
}
