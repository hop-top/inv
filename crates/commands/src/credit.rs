//! `create_credit_note` and `issue_credit_note` commands.
//!
//! Refunds (via `fin.billing.payment.refunded`) auto-create a draft
//! credit note. The operator or agent later issues it via
//! [`issue_credit_note`], which moves Draft → Issued and allocates a
//! `CN-YYYY-NNNN` number.
//!
//! The credit-note FSM (see [`hop_top_inv_core::state::creditnote`]) is
//! `Draft → Issued`. Issued is terminal.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use sqlx::Row;

use hop_top_inv_core::domain::creditnote::{CreditNote, CreditNoteState, CreditNoteStateHistory};
use hop_top_inv_core::domain::ids::{CreditNoteId, HistoryId, InvoiceId};
use hop_top_inv_core::domain::invoice::HistoryChannel;
use hop_top_inv_core::domain::money::Currency;
use hop_top_inv_core::state::creditnote::{
    next_state, CreditNoteEntered, CreditNoteEvent, CreditNoteProposed, CreditNoteTransitioned,
    TOPIC_ENTERED, TOPIC_PROPOSED, TOPIC_TRANSITIONED,
};
use hop_top_inv_store::repo::credit_note::{CreditNoteHistoryRepo, CreditNoteRepo};
use hop_top_inv_store::repo::invoice::InvoiceRepo;

use crate::ctx::{Actor, Channel, CoreCtx};
use crate::draft::history_channel_str;
use crate::error::CoreError;
use crate::publisher::try_publish;

// =============================================================================
// create_credit_note
// =============================================================================

/// Input for [`create_credit_note`].
#[derive(Debug, Clone)]
pub struct CreateCreditNoteInput {
    /// Invoice the credit note credits.
    pub invoice_id: InvoiceId,
    /// Credit amount (positive Decimal, in the invoice's currency).
    pub amount: Decimal,
    /// Free-form reason.
    pub reason: Option<String>,
    /// If auto-created from a refund event, the inbound bus event id.
    pub refund_ref: Option<String>,
    /// Caller-supplied dedupe key.
    pub idempotency_key: Option<String>,
    /// Who triggered.
    pub actor: Actor,
    /// Channel.
    pub channel: Channel,
}

impl CreateCreditNoteInput {
    /// Validate: amount must be positive.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.amount <= Decimal::ZERO {
            return Err(CoreError::Validation(format!(
                "credit-note amount must be > 0, got {}",
                self.amount
            )));
        }
        Ok(())
    }
}

/// Output of [`create_credit_note`].
#[derive(Debug, Clone)]
pub struct CreateCreditNoteOutput {
    /// The newly-created (draft) credit note.
    pub credit_note: CreditNote,
}

/// Create a draft credit note against an invoice.
#[tracing::instrument(skip_all, fields(invoice_id = %input.invoice_id, amount = %input.amount))]
pub async fn create_credit_note(
    ctx: &CoreCtx,
    input: CreateCreditNoteInput,
) -> Result<CreateCreditNoteOutput, CoreError> {
    input.validate()?;

    let inv_repo = InvoiceRepo::new(&ctx.db);

    // Verify the invoice exists + pull its currency.
    let invoice = inv_repo
        .get(&input.invoice_id)
        .await?
        .ok_or_else(|| CoreError::NotFound(format!("invoice {}", input.invoice_id)))?;

    let now = ctx.clock.now();
    let cn = CreditNote {
        id: CreditNoteId::new(),
        number: None,
        invoice_id: input.invoice_id.clone(),
        state: CreditNoteState::Draft,
        amount: invoice.currency.round(input.amount),
        currency: invoice.currency,
        reason: input.reason.clone(),
        refund_ref: input.refund_ref.clone(),
        issued_at: None,
        metadata: BTreeMap::new(),
        created_at: now,
    };

    let channel = HistoryChannel::from(input.channel);
    let history = CreditNoteStateHistory {
        id: HistoryId::new(),
        credit_note_id: cn.id.clone(),
        from_state: None,
        to_state: CreditNoteState::Draft,
        event: "draft".to_string(),
        actor: Some(input.actor.audit_string()),
        channel,
        bus_event_id: input.refund_ref.clone(),
        occurred_at: now,
        published_at: None,
        metadata: BTreeMap::new(),
    };

    {
        let mut tx = ctx
            .db
            .begin()
            .await
            .map_err(hop_top_inv_store::StoreError::from)?;
        CreditNoteRepo::save_in_tx(&mut tx, &cn).await?;
        CreditNoteHistoryRepo::save_in_tx(&mut tx, &history).await?;
        tx.commit()
            .await
            .map_err(hop_top_inv_store::StoreError::from)?;
    }

    try_publish(
        ctx,
        "inv.billing.creditnote.drafted",
        json!({
            "credit_note_id": cn.id.to_string(),
            "invoice_id": cn.invoice_id.to_string(),
            "amount": cn.amount.to_string(),
            "currency": cn.currency.to_string(),
            "refund_ref": cn.refund_ref.clone(),
            "actor": input.actor.audit_string(),
            "channel": history_channel_str(channel),
        }),
        now,
    )
    .await;

    Ok(CreateCreditNoteOutput { credit_note: cn })
}

// =============================================================================
// issue_credit_note
// =============================================================================

/// Input for [`issue_credit_note`].
#[derive(Debug, Clone)]
pub struct IssueCreditNoteInput {
    /// Credit note to issue.
    pub credit_note_id: CreditNoteId,
    /// Caller-supplied dedupe key.
    pub idempotency_key: Option<String>,
    /// Who triggered.
    pub actor: Actor,
    /// Channel.
    pub channel: Channel,
}

impl IssueCreditNoteInput {
    /// No structural validation; the FSM gate is the real check.
    pub fn validate(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

/// Output of [`issue_credit_note`].
#[derive(Debug, Clone)]
pub struct IssueCreditNoteOutput {
    /// The newly-issued credit note (number assigned).
    pub credit_note: CreditNote,
}

/// Transition a draft credit note to issued.
#[tracing::instrument(skip_all, fields(credit_note_id = %input.credit_note_id))]
pub async fn issue_credit_note(
    ctx: &CoreCtx,
    input: IssueCreditNoteInput,
) -> Result<IssueCreditNoteOutput, CoreError> {
    input.validate()?;

    let cn_repo = CreditNoteRepo::new(&ctx.db);

    let mut cn = cn_repo
        .get(&input.credit_note_id)
        .await?
        .ok_or_else(|| CoreError::NotFound(format!("credit note {}", input.credit_note_id)))?;

    let from_state = cn.state;
    let event = CreditNoteEvent::Issue;
    let to_state = next_state(from_state, &event)
        .map_err(|e| CoreError::Validation(format!("credit-note FSM rejected: {e}")))?;
    debug_assert_eq!(to_state, CreditNoteState::Issued);

    let now = ctx.clock.now();
    let year = now.year();
    let count = count_credit_notes_issued_in_year(ctx, year).await?;
    let number = format!("CN-{year:04}-{:04}", count + 1);

    cn.state = to_state;
    cn.number = Some(number.clone());
    cn.issued_at = Some(now);

    let channel = HistoryChannel::from(input.channel);
    let history = CreditNoteStateHistory {
        id: HistoryId::new(),
        credit_note_id: cn.id.clone(),
        from_state: Some(from_state),
        to_state,
        event: event.tag().to_string(),
        actor: Some(input.actor.audit_string()),
        channel,
        bus_event_id: None,
        occurred_at: now,
        published_at: None,
        metadata: BTreeMap::new(),
    };

    {
        let mut tx = ctx
            .db
            .begin()
            .await
            .map_err(hop_top_inv_store::StoreError::from)?;
        CreditNoteRepo::save_in_tx(&mut tx, &cn).await?;
        CreditNoteHistoryRepo::save_in_tx(&mut tx, &history).await?;
        tx.commit()
            .await
            .map_err(hop_top_inv_store::StoreError::from)?;
    }

    publish_creditnote_issued_events(
        ctx,
        &cn,
        from_state,
        to_state,
        &event,
        &input.actor,
        channel,
        now,
    )
    .await;

    Ok(IssueCreditNoteOutput { credit_note: cn })
}

async fn count_credit_notes_issued_in_year(ctx: &CoreCtx, year: i32) -> Result<u32, CoreError> {
    let from = format!("{year:04}-01-01T00:00:00Z");
    let to = format!("{:04}-01-01T00:00:00Z", year + 1);
    let row = sqlx::query(
        "SELECT COUNT(*) AS c FROM credit_notes \
         WHERE issued_at IS NOT NULL AND issued_at >= ? AND issued_at < ?",
    )
    .bind(from)
    .bind(to)
    .fetch_one(&ctx.db)
    .await
    .map_err(hop_top_inv_store::StoreError::from)?;
    let c: i64 = row
        .try_get("c")
        .map_err(hop_top_inv_store::StoreError::from)?;
    Ok(c.max(0) as u32)
}

#[allow(clippy::too_many_arguments)]
async fn publish_creditnote_issued_events(
    ctx: &CoreCtx,
    cn: &CreditNote,
    from: CreditNoteState,
    to: CreditNoteState,
    event: &CreditNoteEvent,
    actor: &Actor,
    channel: HistoryChannel,
    now: DateTime<Utc>,
) {
    let actor_audit = actor.audit_string();
    let proposed = CreditNoteProposed::new(
        cn.id.clone(),
        from,
        to,
        event.clone(),
        Some(actor_audit.clone()),
        channel,
        now,
    );
    let transitioned = CreditNoteTransitioned::new(
        cn.id.clone(),
        from,
        to,
        event.clone(),
        Some(actor_audit.clone()),
        channel,
        now,
    );
    let entered =
        CreditNoteEntered::new(cn.id.clone(), to, channel, Some(actor_audit.clone()), now);

    try_publish(
        ctx,
        TOPIC_PROPOSED,
        serde_json::to_value(&proposed).unwrap_or(json!({})),
        now,
    )
    .await;
    try_publish(
        ctx,
        TOPIC_TRANSITIONED,
        serde_json::to_value(&transitioned).unwrap_or(json!({})),
        now,
    )
    .await;
    try_publish(
        ctx,
        TOPIC_ENTERED,
        serde_json::to_value(&entered).unwrap_or(json!({})),
        now,
    )
    .await;
    try_publish(
        ctx,
        "inv.billing.creditnote.issued",
        json!({
            "credit_note_id": cn.id.to_string(),
            "number": cn.number,
            "invoice_id": cn.invoice_id.to_string(),
            "amount": cn.amount.to_string(),
            "currency": cn.currency.to_string(),
            "actor": actor_audit,
            "channel": history_channel_str(channel),
        }),
        now,
    )
    .await;
}

// Currency is imported but currently unused outside the round() call we use indirectly.
// Suppress the warning at file scope.
#[allow(dead_code)]
const _: Option<Currency> = None;
