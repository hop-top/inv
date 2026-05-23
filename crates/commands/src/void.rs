//! `void_invoice` command — pre-payment void.
//!
//! Allowed from Issued / Sent / Viewed. The FSM rejects Void from
//! PartiallyPaid / Paid / already-Voided — after payment, issue a
//! credit note instead (see [`crate::credit`]).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
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
use crate::events::EmittedEvent;

/// Input for [`void_invoice`].
#[derive(Debug, Clone)]
pub struct VoidInvoiceInput {
    /// Invoice to void.
    pub invoice_id: InvoiceId,
    /// Free-form void reason (recorded on the history row).
    pub reason: Option<String>,
    /// Caller-supplied dedupe key.
    pub idempotency_key: Option<String>,
    /// Who triggered.
    pub actor: Actor,
    /// Channel.
    pub channel: Channel,
}

impl VoidInvoiceInput {
    /// No structural validation needed; the FSM gate is the real check.
    pub fn validate(&self) -> Result<(), CoreError> {
        Ok(())
    }
}

/// Output of [`void_invoice`].
#[derive(Debug, Clone)]
pub struct VoidInvoiceOutput {
    /// Voided invoice.
    pub invoice: Invoice,
    /// Bus events the command would emit.
    pub emitted_events: Vec<EmittedEvent>,
}

/// Void an issued/sent/viewed invoice (pre-payment only).
#[tracing::instrument(skip_all, fields(invoice_id = %input.invoice_id))]
pub async fn void_invoice(
    ctx: &CoreCtx,
    input: VoidInvoiceInput,
) -> Result<VoidInvoiceOutput, CoreError> {
    input.validate()?;

    let inv_repo = InvoiceRepo::new(&ctx.db);

    let mut invoice = inv_repo
        .get(&input.invoice_id)
        .await?
        .ok_or_else(|| CoreError::NotFound(format!("invoice {}", input.invoice_id)))?;

    let from_state = invoice.state;
    let event = InvoiceEvent::Void;
    let to_state = next_state(from_state, &event)?;
    debug_assert_eq!(to_state, InvoiceState::Voided);

    let now = ctx.clock.now();
    invoice.state = to_state;
    invoice.voided_at = Some(now);
    invoice.updated_at = now;

    let channel = HistoryChannel::from(input.channel);
    let history = InvoiceStateHistory {
        id: HistoryId::new(),
        invoice_id: invoice.id.clone(),
        from_state: Some(from_state),
        to_state,
        event: event.tag().to_string(),
        actor: Some(input.actor.audit_string()),
        channel,
        bus_event_id: None,
        reason: input.reason.clone(),
        occurred_at: now,
        published_at: None,
        metadata: BTreeMap::new(),
    };

    {
        let mut tx = ctx.db.begin().await.map_err(inv_store::StoreError::from)?;
        InvoiceRepo::save_in_tx(&mut tx, &invoice).await?;
        InvoiceHistoryRepo::save_in_tx(&mut tx, &history).await?;
        tx.commit().await.map_err(inv_store::StoreError::from)?;
    }

    let emitted = build_voided_events(&invoice, &input, from_state, to_state, &event, channel, now);

    Ok(VoidInvoiceOutput {
        invoice,
        emitted_events: emitted,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_voided_events(
    invoice: &Invoice,
    input: &VoidInvoiceInput,
    from: InvoiceState,
    to: InvoiceState,
    event: &InvoiceEvent,
    channel: HistoryChannel,
    now: DateTime<Utc>,
) -> Vec<EmittedEvent> {
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

    vec![
        EmittedEvent::new(
            TOPIC_PROPOSED,
            serde_json::to_value(&proposed).unwrap_or(json!({})),
            now,
        ),
        EmittedEvent::new(
            TOPIC_TRANSITIONED,
            serde_json::to_value(&transitioned).unwrap_or(json!({})),
            now,
        ),
        EmittedEvent::new(
            TOPIC_ENTERED,
            serde_json::to_value(&entered).unwrap_or(json!({})),
            now,
        ),
        EmittedEvent::new(
            "inv.billing.invoice.voided",
            json!({
                "invoice_id": invoice.id.to_string(),
                "reason": input.reason.clone(),
                "actor": actor_audit,
                "channel": history_channel_str(channel),
            }),
            now,
        ),
    ]
}
