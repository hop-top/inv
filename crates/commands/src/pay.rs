//! `mark_paid` command — record a payment against an issued invoice.
//!
//! Transitions Issued/Sent/Viewed → Paid (full) or PartiallyPaid
//! (partial). PartiallyPaid + remainder → Paid. The FSM gate
//! ([`InvoiceEvent::Pay`]) classifies via cumulative
//! `amount_paid >= total`.
//!
//! When the command is triggered by an inbound bus event
//! (`fin.billing.payment.received`), the caller passes the event id in
//! `bus_event_id` so the history row carries provenance.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde_json::json;

use inv_core::domain::ids::{HistoryId, InvoiceId};
use inv_core::domain::invoice::{HistoryChannel, Invoice, InvoiceState, InvoiceStateHistory};
use inv_core::state::transitions::{next_state, InvoiceEvent};
use inv_core::state::{
    InvoiceEntered, InvoiceProposed, InvoiceTransitioned, TOPIC_ENTERED, TOPIC_PROPOSED,
    TOPIC_TRANSITIONED,
};
use inv_store::repo::history::InvoiceHistoryRepo;
use inv_store::repo::invoice::InvoiceRepo;

use crate::ctx::{Actor, Channel, CoreCtx};
use crate::draft::history_channel_str;
use crate::error::CoreError;
use crate::publisher::try_publish;

/// Input for [`mark_paid`].
#[derive(Debug, Clone)]
pub struct MarkPaidInput {
    /// Invoice receiving the payment.
    pub invoice_id: InvoiceId,
    /// Payment amount. MUST be positive.
    pub amount: Decimal,
    /// When the payment was received (usually from
    /// `fin.billing.payment.received.received_at`). Defaults to
    /// `ctx.clock.now()` when None.
    pub received_at: Option<DateTime<Utc>>,
    /// Caller-supplied dedupe key (advisory; payment events should
    /// also be deduplicated at the bus_inbox layer).
    pub idempotency_key: Option<String>,
    /// Inbound bus event id (when triggered by `fin.billing.payment.received`).
    pub bus_event_id: Option<String>,
    /// Who triggered.
    pub actor: Actor,
    /// Channel.
    pub channel: Channel,
}

impl MarkPaidInput {
    /// Validate: amount must be positive.
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.amount <= Decimal::ZERO {
            return Err(CoreError::Validation(format!(
                "payment amount must be > 0, got {}",
                self.amount
            )));
        }
        Ok(())
    }
}

/// Output of [`mark_paid`].
#[derive(Debug, Clone)]
pub struct MarkPaidOutput {
    /// Invoice with `amount_paid` updated and state transitioned.
    pub invoice: Invoice,
    /// Whether this payment fully settled the invoice.
    pub fully_paid: bool,
}

/// Record a payment against an invoice.
#[tracing::instrument(skip_all, fields(invoice_id = %input.invoice_id, amount = %input.amount))]
pub async fn mark_paid(ctx: &CoreCtx, input: MarkPaidInput) -> Result<MarkPaidOutput, CoreError> {
    input.validate()?;

    let inv_repo = InvoiceRepo::new(&ctx.db);

    // 1. Load invoice.
    let mut invoice = inv_repo
        .get(&input.invoice_id)
        .await?
        .ok_or_else(|| CoreError::NotFound(format!("invoice {}", input.invoice_id)))?;

    // 2. Compute cumulative paid amount + FSM gate.
    let new_amount_paid = invoice.amount_paid + input.amount;
    let from_state = invoice.state;
    let event = InvoiceEvent::Pay {
        amount_paid: new_amount_paid,
        total: invoice.total,
    };
    let to_state = next_state(from_state, &event)?;
    let fully_paid = matches!(to_state, InvoiceState::Paid);

    // 3. Mutate invoice.
    let now = input.received_at.unwrap_or_else(|| ctx.clock.now());
    invoice.amount_paid = new_amount_paid;
    invoice.state = to_state;
    invoice.updated_at = now;
    if fully_paid {
        invoice.paid_at = Some(now);
    }

    // 4. Build history row (also doubles as transactional outbox).
    let channel = HistoryChannel::from(input.channel);
    let mut metadata = BTreeMap::new();
    metadata.insert("amount".to_string(), input.amount.to_string());
    let history = InvoiceStateHistory {
        id: HistoryId::new(),
        invoice_id: invoice.id.clone(),
        from_state: Some(from_state),
        to_state,
        event: event.tag().to_string(),
        actor: Some(input.actor.audit_string()),
        channel,
        bus_event_id: input.bus_event_id.clone(),
        reason: None,
        occurred_at: now,
        published_at: None,
        metadata,
    };

    // 5. Persist mutation + history in one transaction.
    {
        let mut tx = ctx.db.begin().await.map_err(inv_store::StoreError::from)?;
        InvoiceRepo::save_in_tx(&mut tx, &invoice).await?;
        InvoiceHistoryRepo::save_in_tx(&mut tx, &history).await?;
        tx.commit().await.map_err(inv_store::StoreError::from)?;
    }

    // 6. Synchronously publish (T-0043).
    publish_paid_events(
        ctx, &invoice, &input, from_state, to_state, &event, channel, now, fully_paid,
    )
    .await;

    Ok(MarkPaidOutput {
        invoice,
        fully_paid,
    })
}

#[allow(clippy::too_many_arguments)]
async fn publish_paid_events(
    ctx: &CoreCtx,
    invoice: &Invoice,
    input: &MarkPaidInput,
    from: InvoiceState,
    to: InvoiceState,
    event: &InvoiceEvent,
    channel: HistoryChannel,
    now: DateTime<Utc>,
    fully_paid: bool,
) {
    let actor_audit = input.actor.audit_string();

    let proposed = InvoiceProposed::new(
        invoice.id.clone(),
        from,
        to,
        event.clone(),
        Some(actor_audit.clone()),
        channel,
        now,
    );
    let transitioned = InvoiceTransitioned::new(
        invoice.id.clone(),
        from,
        to,
        event.clone(),
        Some(actor_audit.clone()),
        channel,
        now,
    );
    let entered = InvoiceEntered::new(
        invoice.id.clone(),
        to,
        channel,
        Some(actor_audit.clone()),
        now,
    );

    let domain_topic = if fully_paid {
        "inv.billing.invoice.paid"
    } else {
        "inv.billing.invoice.partially_paid"
    };

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
        domain_topic,
        json!({
            "invoice_id": invoice.id.to_string(),
            "amount": input.amount.to_string(),
            "amount_paid": invoice.amount_paid.to_string(),
            "total": invoice.total.to_string(),
            "currency": invoice.currency.to_string(),
            "actor": actor_audit,
            "channel": history_channel_str(channel),
            "bus_event_id": input.bus_event_id.clone(),
        }),
        now,
    )
    .await;
}
