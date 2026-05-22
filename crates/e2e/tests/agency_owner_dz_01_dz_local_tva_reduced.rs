//! Story agency-owner-dz-01: DZ → DZ invoice resolves TVA with a
//! reduced-rate line.
//!
//! Surfaces: commands (draft + issue), tax engine (resolve_tax on
//! domestic_local scope), store (tax_total + tax_rate_ids per line),
//! render (DZD totals, 0 decimals).
//!
//! Per-line tax_category at v1 isn't settable via CLI — uses commands
//! directly (HTTP / WS / MCP would round-trip the same way).
//!
//! xrr: NOT used. Pure compute + store.

mod common;

use std::str::FromStr;

use rust_decimal::Decimal;

use inv_commands::{
    draft_invoice, issue_invoice, Actor, Channel, DraftInvoiceInput, DraftLineInput,
    IssueInvoiceInput,
};
use inv_core::domain::invoice::TaxCategory;
use inv_core::domain::jurisdiction::Jurisdiction;
use inv_core::domain::money::Currency;

#[tokio::test]
async fn dz_local_invoice_resolves_standard_and_reduced_tva() {
    let (ctx, pool, _captured, _blob) = common::fresh_ctx().await;
    let cust = common::seed_customer_dz(&pool).await;

    // ----- When: draft a DZD invoice with one standard + one reduced line.
    let drafted = draft_invoice(
        &ctx,
        DraftInvoiceInput {
            customer_id: cust.clone(),
            seller_jurisdiction: Jurisdiction::AlgiersDz,
            currency: Currency::DZD,
            lines: vec![
                DraftLineInput {
                    description: "Consulting".into(),
                    quantity: Decimal::from_str("1").unwrap(),
                    unit_price: Decimal::from_str("100000").unwrap(),
                    tax_category: TaxCategory::Standard,
                },
                DraftLineInput {
                    description: "Educational material".into(),
                    quantity: Decimal::from_str("1").unwrap(),
                    unit_price: Decimal::from_str("50000").unwrap(),
                    tax_category: TaxCategory::Reduced,
                },
            ],
            idempotency_key: None,
            actor: Actor::Cli { name: "agency".into() },
            channel: Channel::Cli,
            due_at: None,
            template_path: None,
            schedule_id: None,
        },
    )
    .await
    .expect("draft");

    // ----- Then: subtotal = 150000 DZD, tax frozen at issue. ---------
    assert_eq!(drafted.invoice.subtotal, Decimal::from_str("150000").unwrap());
    assert_eq!(drafted.invoice.tax_total, Decimal::ZERO);

    // ----- When: issue. ----------------------------------------------
    let issued = issue_invoice(
        &ctx,
        IssueInvoiceInput {
            invoice_id: drafted.invoice.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli { name: "agency".into() },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("issue");

    // ----- Then: standard 19% + reduced 9% on the right lines. -------
    // Line 1: 100000 * 0.19 = 19000.
    // Line 2:  50000 * 0.09 =  4500.
    // total tax = 23500, total = 173500, DZD scale 0.
    assert_eq!(issued.invoice.tax_total, Decimal::from_str("23500").unwrap());
    assert_eq!(issued.invoice.total, Decimal::from_str("173500").unwrap());

    // Per-line audit: each line records the rate id that was applied.
    let l1 = issued
        .lines
        .iter()
        .find(|l| l.description == "Consulting")
        .expect("line 1");
    let l2 = issued
        .lines
        .iter()
        .find(|l| l.description == "Educational material")
        .expect("line 2");
    assert!(
        l1.tax_rate_ids.iter().any(|r| r == "dz-tva-standard"),
        "line 1 rate ids: {:?}",
        l1.tax_rate_ids
    );
    assert!(
        l2.tax_rate_ids.iter().any(|r| r == "dz-tva-reduced"),
        "line 2 rate ids: {:?}",
        l2.tax_rate_ids
    );
}
