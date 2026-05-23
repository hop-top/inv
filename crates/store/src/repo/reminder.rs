//! Reminder repository.

use sqlx::Row;

use inv_core::domain::ids::{InvoiceId, ReminderId};
use inv_core::domain::reminder::{Reminder, ReminderChannel, ReminderState};

use super::{parse_ts, ts_to_string};
use crate::error::{Result, StoreError};
use crate::pool::Pool;

/// Reminder repository.
#[derive(Debug, Clone)]
pub struct ReminderRepo<'p> {
    pool: &'p Pool,
}

impl<'p> ReminderRepo<'p> {
    /// Construct.
    pub fn new(pool: &'p Pool) -> Self {
        Self { pool }
    }

    /// Upsert. Uses `INSERT ... ON CONFLICT DO UPDATE`.
    pub async fn save(&self, r: &Reminder) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::save_in_tx(&mut tx, r).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Upsert inside a caller-owned transaction (see [`InvoiceRepo::save_in_tx`]).
    pub async fn save_in_tx(tx: &mut sqlx::Transaction<'_, sqlx::Any>, r: &Reminder) -> Result<()> {
        sqlx::query(
            "INSERT INTO reminders (id, invoice_id, scheduled_at, sent_at, channel, state) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT (id) DO UPDATE SET \
              invoice_id = excluded.invoice_id, \
              scheduled_at = excluded.scheduled_at, \
              sent_at = excluded.sent_at, \
              channel = excluded.channel, \
              state = excluded.state",
        )
        .bind(r.id.to_string())
        .bind(r.invoice_id.to_string())
        .bind(ts_to_string(&r.scheduled_at))
        .bind(r.sent_at.as_ref().map(ts_to_string))
        .bind(channel_to_str(r.channel))
        .bind(state_to_str(r.state))
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// Get by id.
    pub async fn get(&self, id: &ReminderId) -> Result<Option<Reminder>> {
        let row = sqlx::query(
            "SELECT id, invoice_id, scheduled_at, sent_at, channel, state \
             FROM reminders WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(self.pool)
        .await?;
        row.as_ref().map(row_to_reminder).transpose()
    }

    /// List reminders for a given invoice.
    pub async fn list_for_invoice(&self, invoice_id: &InvoiceId) -> Result<Vec<Reminder>> {
        let rows = sqlx::query(
            "SELECT id, invoice_id, scheduled_at, sent_at, channel, state \
             FROM reminders WHERE invoice_id = ? ORDER BY scheduled_at ASC",
        )
        .bind(invoice_id.to_string())
        .fetch_all(self.pool)
        .await?;
        rows.iter().map(row_to_reminder).collect()
    }

    /// Pending reminders ready for dispatch (scheduled AND scheduled_at <= cutoff).
    pub async fn due(&self, cutoff: chrono::DateTime<chrono::Utc>) -> Result<Vec<Reminder>> {
        let rows = sqlx::query(
            "SELECT id, invoice_id, scheduled_at, sent_at, channel, state \
             FROM reminders WHERE state = 'scheduled' AND scheduled_at <= ? \
             ORDER BY scheduled_at ASC",
        )
        .bind(ts_to_string(&cutoff))
        .fetch_all(self.pool)
        .await?;
        rows.iter().map(row_to_reminder).collect()
    }
}

fn row_to_reminder(row: &sqlx::any::AnyRow) -> Result<Reminder> {
    let id_s: String = row.try_get("id")?;
    let id = id_s
        .parse::<ReminderId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let invoice_id_s: String = row.try_get("invoice_id")?;
    let invoice_id = invoice_id_s
        .parse::<InvoiceId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let scheduled_at_s: String = row.try_get("scheduled_at")?;
    let sent_at_s: Option<String> = row.try_get("sent_at")?;
    let channel_s: String = row.try_get("channel")?;
    let state_s: String = row.try_get("state")?;

    Ok(Reminder {
        id,
        invoice_id,
        scheduled_at: parse_ts(&scheduled_at_s)?,
        sent_at: sent_at_s.as_deref().map(parse_ts).transpose()?,
        channel: channel_from_str(&channel_s)?,
        state: state_from_str(&state_s)?,
    })
}

fn channel_to_str(c: ReminderChannel) -> &'static str {
    match c {
        ReminderChannel::File => "file",
        ReminderChannel::Stdout => "stdout",
        ReminderChannel::Bus => "bus",
        ReminderChannel::Webhook => "webhook",
        ReminderChannel::Link => "link",
    }
}

fn channel_from_str(s: &str) -> Result<ReminderChannel> {
    Ok(match s {
        "file" => ReminderChannel::File,
        "stdout" => ReminderChannel::Stdout,
        "bus" => ReminderChannel::Bus,
        "webhook" => ReminderChannel::Webhook,
        "link" => ReminderChannel::Link,
        other => {
            return Err(StoreError::InvalidValue {
                column: "reminders.channel",
                value: other.to_string(),
            })
        }
    })
}

fn state_to_str(s: ReminderState) -> &'static str {
    match s {
        ReminderState::Scheduled => "scheduled",
        ReminderState::Sent => "sent",
        ReminderState::Cancelled => "cancelled",
    }
}

fn state_from_str(s: &str) -> Result<ReminderState> {
    Ok(match s {
        "scheduled" => ReminderState::Scheduled,
        "sent" => ReminderState::Sent,
        "cancelled" => ReminderState::Cancelled,
        other => {
            return Err(StoreError::InvalidValue {
                column: "reminders.state",
                value: other.to_string(),
            })
        }
    })
}
