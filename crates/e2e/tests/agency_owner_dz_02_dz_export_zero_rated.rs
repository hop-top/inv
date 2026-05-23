//! Story agency-owner-dz-02: DZ → foreign-buyer invoice resolves as
//! zero-rated export.
//!
//! Surfaces: commands (draft + issue), tax engine (resolve_tax on
//! export scope), store (tax_total = 0).
//!
//! Deviation: the story's example uses EUR, but `hop-top-inv-commands` validates
//! `currency ∈ {USD, CAD, DZD}` at v1 (see crates/commands/src/draft.rs).
//! USD exercises the same export-scope code path (buyer.country != seller
//! country drives `export` scope, not the currency) so we use USD.

mod common;

use std::str::FromStr;

use rust_decimal::Decimal;

use hop_top_inv_commands::{
    draft_invoice, issue_invoice, Actor, Channel, DraftInvoiceInput, DraftLineInput,
    IssueInvoiceInput,
};
use hop_top_inv_core::domain::invoice::TaxCategory;
use hop_top_inv_core::domain::jurisdiction::Jurisdiction;
use hop_top_inv_core::domain::money::Currency;

#[tokio::test]
async fn dz_to_foreign_buyer_resolves_zero_rated_export() {
    let (ctx, pool, _captured, _blob) = common::fresh_ctx().await;
    // Foreign buyer (US); see common::seed_customer_foreign for the
    // EUR-vs-USD deviation rationale.
    let cust = common::seed_customer_foreign(&pool).await;

    // ----- When: draft USD invoice from DZ-16 seller. ----------------
    let drafted = draft_invoice(
        &ctx,
        DraftInvoiceInput {
            customer_id: cust.clone(),
            seller_jurisdiction: Jurisdiction::AlgiersDz,
            currency: Currency::USD,
            lines: vec![DraftLineInput {
                description: "Strategy workshop".into(),
                quantity: Decimal::from_str("1").unwrap(),
                unit_price: Decimal::from_str("5000.00").unwrap(),
                tax_category: TaxCategory::Standard,
            }],
            idempotency_key: None,
            actor: Actor::Cli {
                name: "agency".into(),
            },
            channel: Channel::Cli,
            due_at: None,
            template_path: None,
            schedule_id: None,
        },
    )
    .await
    .expect("draft");

    assert_eq!(
        drafted.invoice.subtotal,
        Decimal::from_str("5000.00").unwrap()
    );

    // ----- When: issue. ----------------------------------------------
    let issued = issue_invoice(
        &ctx,
        IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli {
                name: "agency".into(),
            },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("issue");

    // ----- Then: 0% TVA, total == subtotal, no rate ids on the line
    // (export scope = no matching row recorded, distinguishing it from
    // an explicit `zero_rated` category which DOES record the row id).
    assert_eq!(issued.invoice.tax_total, Decimal::from_str("0.00").unwrap());
    assert_eq!(issued.invoice.total, Decimal::from_str("5000.00").unwrap());
    let line = &issued.lines[0];
    assert_eq!(line.tax_amount, Decimal::from_str("0.00").unwrap());
    // For an export-scope resolution (no explicit zero_rated category),
    // the resolver doesn't pin a specific row id — the tax_rate_ids
    // vec is empty.
    assert!(
        line.tax_rate_ids.is_empty(),
        "export-scope line should not record rate ids, got {:?}",
        line.tax_rate_ids
    );
}

#[tokio::test]
async fn dz_to_foreign_with_explicit_zero_rated_records_rate_id() {
    // Second clause of the story: when the operator explicitly tags
    // the line zero_rated AND a matching row exists, the row id IS
    // recorded for audit, distinguishing "zero-rated because export"
    // from "zero-rated because configured as such".
    let (ctx, pool, _captured, _blob) = common::fresh_ctx().await;
    let cust = common::seed_customer_foreign(&pool).await;

    let drafted = draft_invoice(
        &ctx,
        DraftInvoiceInput {
            customer_id: cust.clone(),
            seller_jurisdiction: Jurisdiction::AlgiersDz,
            currency: Currency::USD,
            lines: vec![DraftLineInput {
                description: "Strategy workshop".into(),
                quantity: Decimal::from_str("1").unwrap(),
                unit_price: Decimal::from_str("5000.00").unwrap(),
                tax_category: TaxCategory::ZeroRated,
            }],
            idempotency_key: None,
            actor: Actor::Cli {
                name: "agency".into(),
            },
            channel: Channel::Cli,
            due_at: None,
            template_path: None,
            schedule_id: None,
        },
    )
    .await
    .expect("draft");
    let issued = issue_invoice(
        &ctx,
        IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli {
                name: "agency".into(),
            },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("issue");
    let line = &issued.lines[0];
    assert_eq!(line.tax_amount, Decimal::from_str("0.00").unwrap());
    assert!(
        line.tax_rate_ids.iter().any(|r| r == "dz-tva-export-zero"),
        "expected dz-tva-export-zero rate id, got {:?}",
        line.tax_rate_ids
    );
}
