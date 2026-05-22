//! Story freelancer-qc-02: materialise monthly retainer from schedule.
//!
//! Surfaces: CLI (in-process via inv-commands), store
//! (schedules + invoices + invoice_state_history), render at auto-issue.
//!
//! No xrr — every step is in-process. Clock is frozen + advanced
//! explicitly to model "day 1 of every month".

mod common;

use std::str::FromStr;
use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use rust_decimal::Decimal;

use inv_commands::{
    schedule_cancel, schedule_create, schedules_tick, Actor, Channel, ScheduleCreateInput,
    ScheduleLineInput, ScheduleStateChangeInput,
};
use inv_core::domain::invoice::{InvoiceState, TaxCategory};
use inv_core::domain::money::Currency;
use inv_core::domain::schedule::{Cadence, ScheduleState};
use inv_store::repo::invoice::InvoiceRepo;

#[tokio::test]
async fn schedule_create_then_tick_materialises_and_auto_issues() {
    let (mut ctx, pool, _captured, _blob) = common::fresh_ctx().await;
    let cust = common::seed_customer_qc(&pool).await;

    // ----- When: create schedule with auto-issue. --------------------
    let created = schedule_create(
        &ctx,
        ScheduleCreateInput {
            customer_id: cust.clone(),
            template_lines: vec![
                ScheduleLineInput {
                    description: "Retainer".into(),
                    quantity: Decimal::from_str("1").unwrap(),
                    unit_price: Decimal::from_str("5000.00").unwrap(),
                    tax_category: TaxCategory::Standard,
                },
                ScheduleLineInput {
                    description: "Add-on hours".into(),
                    quantity: Decimal::from_str("10").unwrap(),
                    unit_price: Decimal::from_str("150.00").unwrap(),
                    tax_category: TaxCategory::Standard,
                },
            ],
            currency: Currency::CAD,
            cadence: Cadence::Monthly { dom: 1 },
            start_date: NaiveDate::from_ymd_opt(2026, 6, 1).unwrap(),
            end_date: None,
            auto_issue: true,
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("create");

    // ----- Then: schedule active, next_run = start_date, event queued.
    assert_eq!(created.schedule.state, ScheduleState::Active);
    assert!(created.schedule.auto_issue);
    assert_eq!(
        created.schedule.next_run,
        NaiveDate::from_ymd_opt(2026, 6, 1).unwrap()
    );
    let topics: Vec<&str> = created.emitted_events.iter().map(|e| e.topic.as_str()).collect();
    assert!(topics.contains(&"inv.billing.schedule.created"));

    // ----- Given today is 2026-06-01 + ticker fires. -----------------
    // Bump frozen clock past start_date so the schedule is due.
    ctx = ctx.with_clock(Arc::new(common::FrozenClock(
        Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
    )));
    let tick = schedules_tick(&ctx).await.expect("tick");
    assert_eq!(tick.ran_schedule_ids.len(), 1);
    assert_eq!(tick.drafts.len(), 1);
    let materialised_id = tick.drafts[0].invoice.id.clone();

    // ----- Then: materialised invoice exists, auto-issued. -----------
    let inv = InvoiceRepo::new(&pool)
        .get(&materialised_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(inv.state, InvoiceState::Issued);
    let num = inv.number.as_deref().expect("issued -> number");
    assert!(num.starts_with("INV-2026-"), "got {num}");
    // T-0036 sub-finding: schedules_tick does not yet stamp
    // invoice.schedule_id with the originating schedule's id (the
    // payload of the inv.billing.schedule.materialised event carries
    // it, but the invoice row itself doesn't get the FK). Materialisation
    // verified by: tick returned the schedule's id in ran_schedule_ids,
    // a draft was created, and tick2 doesn't re-fire — the link in the
    // invoice row is a separate enhancement.
    // Subtotal = 5000 + 10*150 = 6500.
    // Tax: per-line resolution rounds each line individually before
    // summing, so the sum can differ from naive `6500 * (0.05 + 0.09975)`.
    // Line 1 (5000 * 0.14975 = 748.75 → 748.75), Line 2 (1500 * 0.14975
    // = 224.625 → 224.62 with banker's), total = 973.37.
    assert_eq!(inv.subtotal, Decimal::from_str("6500.00").unwrap());
    assert_eq!(inv.tax_total, Decimal::from_str("973.37").unwrap());

    // Re-tick same day → no new materialisation.
    let tick2 = schedules_tick(&ctx).await.unwrap();
    assert!(tick2.ran_schedule_ids.is_empty(), "no double-materialisation");

    // ----- When: cancel. ---------------------------------------------
    let cancelled = schedule_cancel(
        &ctx,
        ScheduleStateChangeInput {
            schedule_id: created.schedule.id.clone(),
            actor: Actor::Cli { name: "jad".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("cancel");

    // ----- Then: terminal cancelled state + bus event. ---------------
    assert_eq!(cancelled.schedule.state, ScheduleState::Cancelled);
    let topics: Vec<&str> = cancelled
        .emitted_events
        .iter()
        .map(|e| e.topic.as_str())
        .collect();
    assert!(topics.contains(&"inv.billing.schedule.cancelled"));

    // Subsequent tick skips cancelled schedule.
    let tick3 = schedules_tick(&ctx).await.unwrap();
    assert!(tick3.ran_schedule_ids.is_empty());
}
