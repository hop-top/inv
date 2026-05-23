//! Story agency-owner-dz-03: credit note against a paid invoice after
//! a dispute.
//!
//! Surfaces: commands (void_invoice rejects, create_credit_note +
//! issue_credit_note), store (credit_notes + history), bus emission.
//!
//! xrr: NOT used. Pure in-process FSM + store.

mod common;

use std::str::FromStr;

use rust_decimal::Decimal;

use inv_commands::{
    create_credit_note, draft_invoice, issue_credit_note, issue_invoice, mark_paid, void_invoice,
    Actor, Channel, CoreError, CreateCreditNoteInput, DraftInvoiceInput, DraftLineInput,
    IssueCreditNoteInput, IssueInvoiceInput, MarkPaidInput, VoidInvoiceInput,
};
use inv_core::domain::creditnote::CreditNoteState;
use inv_core::domain::invoice::{InvoiceState, TaxCategory};
use inv_core::domain::jurisdiction::Jurisdiction;
use inv_core::domain::money::Currency;
use inv_store::repo::credit_note::CreditNoteRepo;
use inv_store::repo::invoice::InvoiceRepo;

#[tokio::test]
async fn void_after_paid_rejected_and_credit_note_lifecycle() {
    let (ctx, pool, _captured, _blob) = common::fresh_ctx().await;
    let cust = common::seed_customer_dz(&pool).await;

    // Build a paid DZ-local invoice (subtotal 150000 + 23500 TVA = 173500).
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
    .unwrap();
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
    .unwrap();
    assert_eq!(issued.invoice.total, Decimal::from_str("173500").unwrap());
    let paid = mark_paid(
        &ctx,
        MarkPaidInput {
            invoice_id: issued.invoice.id.clone(),
            amount: issued.invoice.total,
            received_at: None,
            idempotency_key: None,
            bus_event_id: None,
            actor: Actor::Cli {
                name: "agency".into(),
            },
            channel: Channel::Cli,
        },
    )
    .await
    .unwrap();
    assert!(paid.fully_paid);

    // ----- When: void after payment. ---------------------------------
    let err = void_invoice(
        &ctx,
        VoidInvoiceInput {
            invoice_id: issued.invoice.id.clone(),
            reason: Some("Client dispute".into()),
            idempotency_key: None,
            actor: Actor::Cli {
                name: "agency".into(),
            },
            channel: Channel::Cli,
        },
    )
    .await
    .expect_err("void must reject");

    // ----- Then: FsmTransition + state unchanged. --------------------
    assert!(matches!(err, CoreError::FsmTransition(_)), "got {err:?}");
    let back = InvoiceRepo::new(&pool)
        .get(&issued.invoice.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back.state, InvoiceState::Paid);

    // ----- When: draft + issue a credit note. ------------------------
    let cn_draft = create_credit_note(
        &ctx,
        CreateCreditNoteInput {
            invoice_id: issued.invoice.id.clone(),
            amount: Decimal::from_str("50000").unwrap(),
            reason: Some("Partial refund: under-delivered scope".into()),
            refund_ref: None,
            idempotency_key: None,
            actor: Actor::Cli {
                name: "agency".into(),
            },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("draft cn");
    assert_eq!(cn_draft.credit_note.state, CreditNoteState::Draft);
    let topics: Vec<&str> = cn_draft
        .emitted_events
        .iter()
        .map(|e| e.topic.as_str())
        .collect();
    assert!(topics.contains(&"inv.billing.creditnote.drafted"));

    let cn_issued = issue_credit_note(
        &ctx,
        IssueCreditNoteInput {
            credit_note_id: cn_draft.credit_note.id.clone(),
            idempotency_key: None,
            actor: Actor::Cli {
                name: "agency".into(),
            },
            channel: Channel::Cli,
        },
    )
    .await
    .expect("issue cn");
    assert_eq!(cn_issued.credit_note.state, CreditNoteState::Issued);
    let num = cn_issued.credit_note.number.as_deref().expect("CN number");
    assert!(num.starts_with("CN-2026-"), "got {num}");

    // Credit note persisted at Issued state.
    let back_cn = CreditNoteRepo::new(&pool)
        .get(&cn_draft.credit_note.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back_cn.state, CreditNoteState::Issued);
    assert_eq!(back_cn.amount, Decimal::from_str("50000").unwrap());
    assert_eq!(back_cn.invoice_id, issued.invoice.id);

    // Original invoice's state still Paid (credit notes net for
    // reporting but don't transition the invoice's FSM).
    let back_inv = InvoiceRepo::new(&pool)
        .get(&issued.invoice.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(back_inv.state, InvoiceState::Paid);

    let topics: Vec<&str> = cn_issued
        .emitted_events
        .iter()
        .map(|e| e.topic.as_str())
        .collect();
    for expected in [
        "inv.billing.creditnote.proposed",
        "inv.billing.creditnote.transitioned",
        "inv.billing.creditnote.entered",
        "inv.billing.creditnote.issued",
    ] {
        assert!(
            topics.contains(&expected),
            "missing `{expected}` in {topics:?}"
        );
    }
}
