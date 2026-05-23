//! Snapshot tests for the HTML render pipeline.
//!
//! These exercise:
//!
//! - the bundled default template with a full fixture invoice
//! - empty-lines edge case
//! - draft (no number) edge case
//! - DZD (0-decimal currency) edge case
//! - credit-note metadata field surfacing in the notes section
//! - an override template path
//!
//! Snapshots are committed under `crates/core/tests/snapshots/`. To
//! review or update on local changes:
//!
//! ```text
//! cargo insta review -p hop-top-inv-core
//! ```

use chrono::{TimeZone, Utc};
use hop_top_inv_core::domain::address::Address;
use hop_top_inv_core::domain::customer::Customer;
use hop_top_inv_core::domain::ids::{CustomerId, InvoiceId, LineId};
use hop_top_inv_core::domain::invoice::{Invoice, InvoiceLine, InvoiceState, TaxCategory};
use hop_top_inv_core::domain::jurisdiction::Jurisdiction;
use hop_top_inv_core::domain::money::Currency;
use hop_top_inv_core::render::{render_html, RenderContext};
use rust_decimal::Decimal;
use std::collections::BTreeMap;
use std::str::FromStr;

/// Build a frozen-clock invoice + customer + lines triple for snapshots.
///
/// IDs and timestamps are deterministic so the snapshot output is
/// stable. Anything that depends on `Utc::now()` or `Id::new()` would
/// break snapshot equality.
fn fixture_full() -> (Invoice, Customer, Vec<InvoiceLine>) {
    let inv_id = InvoiceId::parse("invoice_01j5xkb8m0z000000000000000").expect("typeid");
    let cust_id = CustomerId::parse("customer_01j5xkb8m0z000000000000000").expect("typeid");
    let line1_id = LineId::parse("line_01j5xkb8m0z000000000000001").expect("typeid");
    let line2_id = LineId::parse("line_01j5xkb8m0z000000000000002").expect("typeid");

    let frozen = Utc.with_ymd_and_hms(2026, 5, 1, 12, 0, 0).unwrap();
    let due = Utc.with_ymd_and_hms(2026, 5, 31, 12, 0, 0).unwrap();

    let customer = Customer {
        id: cust_id.clone(),
        display_name: "Acme Corp".into(),
        email: Some("billing@acme.example".into()),
        address: Address {
            country: "CA".into(),
            region: Some("QC".into()),
            city: Some("Montréal".into()),
            postal: Some("H2X 1Y4".into()),
            line1: Some("123 Rue Saint-Denis".into()),
            line2: Some("Suite 400".into()),
        },
        metadata: BTreeMap::new(),
        created_at: frozen,
        updated_at: frozen,
    };

    let lines = vec![
        InvoiceLine {
            id: line1_id,
            invoice_id: inv_id.clone(),
            position: 0,
            description: "Consulting hours — May".into(),
            quantity: Decimal::from_str("10").unwrap(),
            unit_price: Decimal::from_str("125.00").unwrap(),
            tax_rate_ids: vec!["ca-qc-gst".into(), "ca-qc-qst".into()],
            tax_category: TaxCategory::Standard,
            tax_amount: Decimal::from_str("187.19").unwrap(),
            line_total: Decimal::from_str("1437.19").unwrap(),
            metadata: BTreeMap::new(),
        },
        InvoiceLine {
            id: line2_id,
            invoice_id: inv_id.clone(),
            position: 1,
            description: "Workshop facilitation".into(),
            quantity: Decimal::from_str("1").unwrap(),
            unit_price: Decimal::from_str("500.00").unwrap(),
            tax_rate_ids: vec!["ca-qc-gst".into(), "ca-qc-qst".into()],
            tax_category: TaxCategory::Standard,
            tax_amount: Decimal::from_str("74.88").unwrap(),
            line_total: Decimal::from_str("574.88").unwrap(),
            metadata: BTreeMap::new(),
        },
    ];

    let mut metadata = BTreeMap::new();
    metadata.insert(
        "notes".to_string(),
        "Net 30. Late fee 1.5% / month.".to_string(),
    );
    metadata.insert("po_number".to_string(), "PO-2026-0042".to_string());

    let invoice = Invoice {
        id: inv_id,
        number: Some("INV-2026-0001".into()),
        customer_id: cust_id,
        seller_jurisdiction: Jurisdiction::QuebecCa,
        currency: Currency::CAD,
        state: InvoiceState::Issued,
        issued_at: Some(frozen),
        due_at: Some(due),
        sent_at: None,
        viewed_at: None,
        paid_at: None,
        voided_at: None,
        subtotal: Decimal::from_str("1750.00").unwrap(),
        tax_total: Decimal::from_str("262.07").unwrap(),
        total: Decimal::from_str("2012.07").unwrap(),
        amount_paid: Decimal::ZERO,
        schedule_id: None,
        template_path: None,
        pdf_blob_ref: None,
        idempotency_key: None,
        nexus_review: false,
        metadata,
        created_at: frozen,
        updated_at: frozen,
    };

    (invoice, customer, lines)
}

#[tokio::test]
async fn renders_full_invoice_with_bundled_template() {
    let (inv, cust, lines) = fixture_full();
    let ctx = RenderContext::new(inv, cust, lines);
    let html = render_html(None, &ctx).await.expect("render ok");
    insta::assert_snapshot!("full_invoice_bundled", html);
}

#[tokio::test]
async fn renders_invoice_with_no_lines() {
    let (mut inv, cust, _) = fixture_full();
    inv.subtotal = Decimal::ZERO;
    inv.tax_total = Decimal::ZERO;
    inv.total = Decimal::ZERO;
    let ctx = RenderContext::new(inv, cust, vec![]);
    let html = render_html(None, &ctx).await.expect("render ok");
    insta::assert_snapshot!("empty_lines_bundled", html);
}

#[tokio::test]
async fn renders_draft_invoice_without_number() {
    let (mut inv, cust, lines) = fixture_full();
    inv.number = None;
    inv.state = InvoiceState::Draft;
    inv.issued_at = None;
    inv.due_at = None;
    let ctx = RenderContext::new(inv, cust, lines);
    let html = render_html(None, &ctx).await.expect("render ok");
    insta::assert_snapshot!("draft_no_number_bundled", html);
}

#[tokio::test]
async fn renders_dzd_zero_decimal_invoice() {
    let (mut inv, mut cust, mut lines) = fixture_full();
    inv.currency = Currency::DZD;
    inv.seller_jurisdiction = Jurisdiction::AlgiersDz;
    inv.subtotal = Decimal::from_str("12000").unwrap();
    inv.tax_total = Decimal::from_str("2280").unwrap();
    inv.total = Decimal::from_str("14280").unwrap();
    for l in lines.iter_mut() {
        l.unit_price = Decimal::from_str("6000").unwrap();
        l.quantity = Decimal::from_str("1").unwrap();
        l.tax_amount = Decimal::from_str("1140").unwrap();
        l.line_total = Decimal::from_str("7140").unwrap();
    }
    lines.truncate(2);
    cust.address = Address {
        country: "DZ".into(),
        region: Some("16".into()),
        city: Some("Alger".into()),
        postal: Some("16000".into()),
        line1: Some("Rue Didouche Mourad".into()),
        line2: None,
    };
    let ctx = RenderContext::new(inv, cust, lines);
    let html = render_html(None, &ctx).await.expect("render ok");
    insta::assert_snapshot!("dzd_zero_decimal_bundled", html);
}

#[tokio::test]
async fn renders_invoice_with_empty_metadata() {
    // Operator may issue an invoice with no metadata at all. The
    // domain type skips serializing an empty metadata bag entirely,
    // so the template must guard `invoice.metadata.*` accesses. This
    // snapshot pins the rendered form (notes + metadata sections
    // both omitted).
    let (mut inv, cust, lines) = fixture_full();
    inv.metadata = BTreeMap::new();
    let ctx = RenderContext::new(inv, cust, lines);
    let html = render_html(None, &ctx).await.expect("render ok");
    insta::assert_snapshot!("empty_metadata_bundled", html);
}

#[tokio::test]
async fn renders_invoice_with_credit_note_metadata() {
    let (mut inv, cust, lines) = fixture_full();
    inv.metadata
        .insert("credit_note_for".to_string(), "INV-2025-0099".to_string());
    inv.metadata.insert(
        "notes".to_string(),
        "This invoice supersedes INV-2025-0099.".to_string(),
    );
    let ctx = RenderContext::new(inv, cust, lines);
    let html = render_html(None, &ctx).await.expect("render ok");
    insta::assert_snapshot!("credit_note_metadata_bundled", html);
}

#[tokio::test]
async fn renders_with_override_template() {
    // Stub template: prove that an alternate filesystem template wins
    // over the bundled default. Kept intentionally trivial to keep the
    // snapshot small + diff-friendly.
    let dir = tempdir_unique();
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("override.html.tera");
    std::fs::write(
        &path,
        "<custom>{{ invoice_label }} | {{ invoice.currency }} | lines={{ lines | length }}</custom>",
    )
    .unwrap();

    let (inv, cust, lines) = fixture_full();
    let ctx = RenderContext::new(inv, cust, lines);
    let html = render_html(Some(&path), &ctx).await.expect("render ok");
    insta::assert_snapshot!("override_template", html);
}

#[tokio::test]
async fn pdf_stub_returns_html_bytes() {
    // With the default `pdf-stub` feature, render_pdf round-trips the
    // HTML bytes. This locks the contract in place so callers can
    // exercise the full pipeline today without a real engine.
    let (inv, cust, lines) = fixture_full();
    let ctx = RenderContext::new(inv, cust, lines);
    let html = render_html(None, &ctx).await.unwrap();
    let pdf = hop_top_inv_core::render::render_pdf(&html).await.unwrap();
    assert_eq!(pdf, html.as_bytes());
}

/// Unique tempdir per test invocation. Avoids cross-test races on
/// shared filesystem names without pulling `tempfile` as a dep.
fn tempdir_unique() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("hop-top-inv-core-render-{pid}-{n}"))
}
