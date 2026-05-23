//! Invoice state-history repository (audit + transactional outbox).

use chrono::Utc;
use sqlx::Row;

use inv_core::domain::ids::{HistoryId, InvoiceId};
use inv_core::domain::invoice::{HistoryChannel, InvoiceStateHistory};

use super::invoice::{state_from_str, state_to_str};
use super::{metadata_from_json, metadata_to_json, parse_ts, ts_to_string};
use crate::error::{Result, StoreError};
use crate::pool::Pool;

/// Invoice history repository.
#[derive(Debug, Clone)]
pub struct InvoiceHistoryRepo<'p> {
    pool: &'p Pool,
}

impl<'p> InvoiceHistoryRepo<'p> {
    /// Construct.
    pub fn new(pool: &'p Pool) -> Self {
        Self { pool }
    }

    /// Append a row. Opens its own short transaction; for atomic
    /// composition with an invoice mutation, use [`Self::save_in_tx`].
    pub async fn save(&self, h: &InvoiceStateHistory) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::save_in_tx(&mut tx, h).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Append a row inside a caller-owned transaction (see
    /// [`crate::repo::invoice::InvoiceRepo::save_in_tx`]).
    pub async fn save_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        h: &InvoiceStateHistory,
    ) -> Result<()> {
        let metadata = metadata_to_json(&h.metadata)?;
        sqlx::query(
            "INSERT INTO invoice_state_history \
             (id, invoice_id, from_state, to_state, event, actor, channel, \
              bus_event_id, reason, occurred_at, published_at, metadata) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(h.id.to_string())
        .bind(h.invoice_id.to_string())
        .bind(h.from_state.map(state_to_str))
        .bind(state_to_str(h.to_state))
        .bind(&h.event)
        .bind(h.actor.clone())
        .bind(channel_to_str(h.channel))
        .bind(h.bus_event_id.clone())
        .bind(h.reason.clone())
        .bind(ts_to_string(&h.occurred_at))
        .bind(h.published_at.as_ref().map(ts_to_string))
        .bind(metadata)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// List history rows for an invoice in occurrence order.
    pub async fn list_for_invoice(
        &self,
        invoice_id: &InvoiceId,
    ) -> Result<Vec<InvoiceStateHistory>> {
        let rows = sqlx::query(
            "SELECT id, invoice_id, from_state, to_state, event, actor, channel, \
                    bus_event_id, reason, occurred_at, published_at, metadata \
             FROM invoice_state_history WHERE invoice_id = ? ORDER BY occurred_at ASC",
        )
        .bind(invoice_id.to_string())
        .fetch_all(self.pool)
        .await?;
        rows.iter().map(row_to_history).collect()
    }

    /// Outbox query: pending rows that need bus publication.
    pub async fn pending_outbox(&self, limit: i64) -> Result<Vec<InvoiceStateHistory>> {
        let rows = sqlx::query(
            "SELECT id, invoice_id, from_state, to_state, event, actor, channel, \
                    bus_event_id, reason, occurred_at, published_at, metadata \
             FROM invoice_state_history WHERE published_at IS NULL \
             ORDER BY occurred_at ASC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(self.pool)
        .await?;
        rows.iter().map(row_to_history).collect()
    }

    /// Mark an outbox row as published.
    pub async fn mark_published(&self, id: &HistoryId) -> Result<()> {
        sqlx::query("UPDATE invoice_state_history SET published_at = ? WHERE id = ?")
            .bind(ts_to_string(&Utc::now()))
            .bind(id.to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

fn row_to_history(row: &sqlx::any::AnyRow) -> Result<InvoiceStateHistory> {
    let id_s: String = row.try_get("id")?;
    let id = id_s
        .parse::<HistoryId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let invoice_id_s: String = row.try_get("invoice_id")?;
    let invoice_id = invoice_id_s
        .parse::<InvoiceId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let from_state_s: Option<String> = row.try_get("from_state")?;
    let from_state = from_state_s.as_deref().map(state_from_str).transpose()?;
    let to_state_s: String = row.try_get("to_state")?;
    let to_state = state_from_str(&to_state_s)?;
    let event: String = row.try_get("event")?;
    let actor: Option<String> = row.try_get("actor")?;
    let channel_s: String = row.try_get("channel")?;
    let channel = channel_from_str(&channel_s)?;
    let bus_event_id: Option<String> = row.try_get("bus_event_id")?;
    let reason: Option<String> = row.try_get("reason")?;
    let occurred_at_s: String = row.try_get("occurred_at")?;
    let published_at_s: Option<String> = row.try_get("published_at")?;
    let metadata_s: Option<String> = row.try_get("metadata")?;

    Ok(InvoiceStateHistory {
        id,
        invoice_id,
        from_state,
        to_state,
        event,
        actor,
        channel,
        bus_event_id,
        reason,
        occurred_at: parse_ts(&occurred_at_s)?,
        published_at: published_at_s.as_deref().map(parse_ts).transpose()?,
        metadata: metadata_from_json(metadata_s.as_deref())?,
    })
}

pub(crate) fn channel_to_str(c: HistoryChannel) -> &'static str {
    match c {
        HistoryChannel::Cli => "cli",
        HistoryChannel::Api => "api",
        HistoryChannel::Ws => "ws",
        HistoryChannel::Mcp => "mcp",
        HistoryChannel::Bus => "bus",
    }
}

pub(crate) fn channel_from_str(s: &str) -> Result<HistoryChannel> {
    Ok(match s {
        "cli" => HistoryChannel::Cli,
        "api" => HistoryChannel::Api,
        "ws" => HistoryChannel::Ws,
        "mcp" => HistoryChannel::Mcp,
        "bus" => HistoryChannel::Bus,
        other => {
            return Err(StoreError::InvalidValue {
                column: "channel",
                value: other.to_string(),
            })
        }
    })
}
