//! `send_idempotency` table — per-(invoice, key) dedupe for
//! `send_invoice` / `send_invoice_render` (see T-0037).
//!
//! One row per first-successful send for a given
//! `(invoice_id, idempotency_key)` tuple. The row is written inside the
//! same sqlx transaction as the `invoice_state_history` `send` row so a
//! crash between FSM write and idempotency write can't poison replay.
//! On replay, [`SendIdempotencyRepo::find`] returns the cached delivery
//! metadata + the originating history id; the caller then short-circuits
//! the FSM transition and skips event emission.

use chrono::{DateTime, Utc};
use sqlx::Row;

use hop_top_inv_core::domain::ids::{HistoryId, InvoiceId};

use super::{parse_ts, ts_to_string};
use crate::error::{Result, StoreError};
use crate::pool::Pool;

/// One row in `send_idempotency`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendIdempotencyRecord {
    /// Invoice the send was for.
    pub invoice_id: InvoiceId,
    /// Caller-supplied dedupe key (scoped per-invoice).
    pub idempotency_key: String,
    /// History row id created by the original send (back-reference, not
    /// a hard FK to keep this table self-contained on cleanup).
    pub history_id: HistoryId,
    /// Resolved destination string recorded on the original send.
    pub delivered_to: String,
    /// When the original send committed.
    pub created_at: DateTime<Utc>,
}

/// Repo for the `send_idempotency` table.
#[derive(Debug, Clone)]
pub struct SendIdempotencyRepo<'p> {
    pool: &'p Pool,
}

impl<'p> SendIdempotencyRepo<'p> {
    /// Construct.
    pub fn new(pool: &'p Pool) -> Self {
        Self { pool }
    }

    /// Look up a prior send by `(invoice_id, idempotency_key)`.
    /// Returns `None` when no prior send was recorded under this tuple.
    pub async fn find(
        &self,
        invoice_id: &InvoiceId,
        idempotency_key: &str,
    ) -> Result<Option<SendIdempotencyRecord>> {
        let row = sqlx::query(
            "SELECT invoice_id, idempotency_key, history_id, delivered_to, created_at \
             FROM send_idempotency WHERE invoice_id = ? AND idempotency_key = ?",
        )
        .bind(invoice_id.to_string())
        .bind(idempotency_key)
        .fetch_optional(self.pool)
        .await?;
        row.as_ref().map(row_to_record).transpose()
    }

    /// Insert a new row inside a caller-owned transaction. Returns an
    /// error if the `(invoice_id, idempotency_key)` already exists; the
    /// caller is expected to have checked [`Self::find`] first.
    pub async fn insert_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        rec: &SendIdempotencyRecord,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO send_idempotency \
             (invoice_id, idempotency_key, history_id, delivered_to, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(rec.invoice_id.to_string())
        .bind(&rec.idempotency_key)
        .bind(rec.history_id.to_string())
        .bind(&rec.delivered_to)
        .bind(ts_to_string(&rec.created_at))
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
}

fn row_to_record(row: &sqlx::any::AnyRow) -> Result<SendIdempotencyRecord> {
    let invoice_id_s: String = row.try_get("invoice_id")?;
    let invoice_id = invoice_id_s
        .parse::<InvoiceId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let idempotency_key: String = row.try_get("idempotency_key")?;
    let history_id_s: String = row.try_get("history_id")?;
    let history_id = history_id_s
        .parse::<HistoryId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let delivered_to: String = row.try_get("delivered_to")?;
    let created_at_s: String = row.try_get("created_at")?;
    Ok(SendIdempotencyRecord {
        invoice_id,
        idempotency_key,
        history_id,
        delivered_to,
        created_at: parse_ts(&created_at_s)?,
    })
}
