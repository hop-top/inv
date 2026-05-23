//! Inbound bus-event de-dup table.
//!
//! No domain type for this row yet (T-0014 will likely add one to
//! `hop-top-inv-bus`); we ship a struct local to this crate.

use chrono::{DateTime, Utc};
use sqlx::Row;

use hop_top_inv_core::domain::ids::InvoiceId;

use super::{parse_ts, ts_to_string};
use crate::error::{Result, StoreError};
use crate::pool::Pool;

/// One row in `bus_inbox`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusInboxRecord {
    /// Inbound event id (primary key, used to de-dup replays).
    pub event_id: String,
    /// Bus topic.
    pub topic: String,
    /// Originating source (matches the first segment of the topic).
    pub source: String,
    /// When `inv` received the event.
    pub received_at: DateTime<Utc>,
    /// Payload, stored verbatim as JSON.
    pub payload_json: String,
    /// Set when the consumer finished processing.
    pub processed_at: Option<DateTime<Utc>>,
    /// Back-link to an invoice if the event affected one.
    pub invoice_id: Option<InvoiceId>,
}

/// Bus-inbox repository.
#[derive(Debug, Clone)]
pub struct BusInboxRepo<'p> {
    pool: &'p Pool,
}

impl<'p> BusInboxRepo<'p> {
    /// Construct.
    pub fn new(pool: &'p Pool) -> Self {
        Self { pool }
    }

    /// Insert. Returns `false` if the row already existed (de-dup hit).
    pub async fn try_insert(&self, rec: &BusInboxRecord) -> Result<bool> {
        // Check first; INSERT OR IGNORE would be portable on sqlite but not
        // on postgres/mysql. Two-step is portable.
        let existing = sqlx::query("SELECT event_id FROM bus_inbox WHERE event_id = ?")
            .bind(&rec.event_id)
            .fetch_optional(self.pool)
            .await?;
        if existing.is_some() {
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO bus_inbox \
             (event_id, topic, source, received_at, payload_json, processed_at, invoice_id) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&rec.event_id)
        .bind(&rec.topic)
        .bind(&rec.source)
        .bind(ts_to_string(&rec.received_at))
        .bind(&rec.payload_json)
        .bind(rec.processed_at.as_ref().map(ts_to_string))
        .bind(rec.invoice_id.as_ref().map(|i| i.to_string()))
        .execute(self.pool)
        .await?;
        Ok(true)
    }

    /// Mark a row as processed (sets `processed_at` to now).
    pub async fn mark_processed(&self, event_id: &str) -> Result<()> {
        sqlx::query("UPDATE bus_inbox SET processed_at = ? WHERE event_id = ?")
            .bind(ts_to_string(&Utc::now()))
            .bind(event_id)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// Get by id.
    pub async fn get(&self, event_id: &str) -> Result<Option<BusInboxRecord>> {
        let row = sqlx::query(
            "SELECT event_id, topic, source, received_at, payload_json, processed_at, invoice_id \
             FROM bus_inbox WHERE event_id = ?",
        )
        .bind(event_id)
        .fetch_optional(self.pool)
        .await?;
        row.as_ref().map(row_to_inbox).transpose()
    }
}

fn row_to_inbox(row: &sqlx::any::AnyRow) -> Result<BusInboxRecord> {
    let event_id: String = row.try_get("event_id")?;
    let topic: String = row.try_get("topic")?;
    let source: String = row.try_get("source")?;
    let received_at_s: String = row.try_get("received_at")?;
    let payload_json: String = row.try_get("payload_json")?;
    let processed_at_s: Option<String> = row.try_get("processed_at")?;
    let invoice_id_s: Option<String> = row.try_get("invoice_id")?;
    let invoice_id = match invoice_id_s {
        None => None,
        Some(s) => Some(
            s.parse::<InvoiceId>()
                .map_err(|e| StoreError::Id(e.to_string()))?,
        ),
    };
    Ok(BusInboxRecord {
        event_id,
        topic,
        source,
        received_at: parse_ts(&received_at_s)?,
        payload_json,
        processed_at: processed_at_s.as_deref().map(parse_ts).transpose()?,
        invoice_id,
    })
}
