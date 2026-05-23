//! Credit-note repository + credit-note state-history repository.

use chrono::Utc;
use sqlx::Row;

use hop_top_inv_core::domain::creditnote::{CreditNote, CreditNoteState, CreditNoteStateHistory};
use hop_top_inv_core::domain::ids::{CreditNoteId, HistoryId, InvoiceId};
use hop_top_inv_core::domain::invoice::HistoryChannel;
use hop_top_inv_core::domain::money::Currency;

use super::{
    decimal_to_string, metadata_from_json, metadata_to_json, parse_decimal, parse_ts, ts_to_string,
};
use crate::error::{Result, StoreError};
use crate::pool::Pool;
use crate::sql::{portable_sql, portable_sql_for_tx};

/// Filter for listing credit notes.
#[derive(Debug, Default, Clone)]
pub struct CreditNoteFilter {
    /// Restrict to credit notes against a specific invoice.
    pub invoice_id: Option<InvoiceId>,
    /// Restrict to a specific lifecycle state.
    pub state: Option<CreditNoteState>,
    /// Max rows.
    pub limit: Option<i64>,
    /// Offset for paging.
    pub offset: Option<i64>,
}

/// Credit-note repository.
#[derive(Debug, Clone)]
pub struct CreditNoteRepo<'p> {
    pool: &'p Pool,
}

impl<'p> CreditNoteRepo<'p> {
    /// Construct.
    pub fn new(pool: &'p Pool) -> Self {
        Self { pool }
    }

    /// Upsert. Uses `INSERT ... ON CONFLICT DO UPDATE` so existing
    /// rows are updated in place, preserving CASCADEing children
    /// like `credit_note_state_history`.
    pub async fn save(&self, n: &CreditNote) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::save_in_tx(&mut tx, n).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Upsert inside a caller-owned transaction (see [`InvoiceRepo::save_in_tx`]).
    pub async fn save_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        n: &CreditNote,
    ) -> Result<()> {
        let metadata = metadata_to_json(&n.metadata)?;
        let sql = portable_sql_for_tx(
            tx,
            "INSERT INTO credit_notes \
             (id, number, invoice_id, state, amount, currency, reason, refund_ref, issued_at, created_at, metadata) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (id) DO UPDATE SET \
              number = excluded.number, \
              invoice_id = excluded.invoice_id, \
              state = excluded.state, \
              amount = excluded.amount, \
              currency = excluded.currency, \
              reason = excluded.reason, \
              refund_ref = excluded.refund_ref, \
              issued_at = excluded.issued_at, \
              metadata = excluded.metadata",
        );
        sqlx::query(&sql)
            .bind(n.id.to_string())
            .bind(n.number.clone())
            .bind(n.invoice_id.to_string())
            .bind(cn_state_to_str(n.state))
            .bind(decimal_to_string(&n.amount))
            .bind(n.currency.to_string())
            .bind(n.reason.clone())
            .bind(n.refund_ref.clone())
            .bind(n.issued_at.as_ref().map(ts_to_string))
            .bind(ts_to_string(&n.created_at))
            .bind(metadata)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    /// Fetch by id.
    pub async fn get(&self, id: &CreditNoteId) -> Result<Option<CreditNote>> {
        let sql = portable_sql(
            self.pool,
            "SELECT id, number, invoice_id, state, amount, currency, reason, refund_ref, issued_at, created_at, metadata \
             FROM credit_notes WHERE id = ?",
        )
        .await?;
        let row = sqlx::query(&sql)
            .bind(id.to_string())
            .fetch_optional(self.pool)
            .await?;
        row.as_ref().map(row_to_credit_note).transpose()
    }

    /// List with filters. With no filter set, returns every credit
    /// note across all invoices ordered by `created_at ASC`.
    pub async fn list(&self, filter: &CreditNoteFilter) -> Result<Vec<CreditNote>> {
        // Build the WHERE dynamically. The SQL is assembled in sqlite's
        // `?` form, then [`portable_sql`] rewrites to `$N` for postgres
        // immediately before query() consumes it.
        let mut sql = String::from(
            "SELECT id, number, invoice_id, state, amount, currency, reason, refund_ref, issued_at, created_at, metadata \
             FROM credit_notes WHERE 1=1",
        );
        if filter.invoice_id.is_some() {
            sql.push_str(" AND invoice_id = ?");
        }
        if filter.state.is_some() {
            sql.push_str(" AND state = ?");
        }
        sql.push_str(" ORDER BY created_at ASC");
        if filter.limit.is_some() {
            sql.push_str(" LIMIT ?");
        }
        if filter.offset.is_some() {
            sql.push_str(" OFFSET ?");
        }
        let sql = portable_sql(self.pool, &sql).await?;

        let mut q = sqlx::query(&sql);
        if let Some(i) = filter.invoice_id.as_ref() {
            q = q.bind(i.to_string());
        }
        if let Some(s) = filter.state {
            q = q.bind(cn_state_to_str(s));
        }
        if let Some(l) = filter.limit {
            q = q.bind(l);
        }
        if let Some(o) = filter.offset {
            q = q.bind(o);
        }
        let rows = q.fetch_all(self.pool).await?;
        rows.iter().map(row_to_credit_note).collect()
    }

    /// List credit notes against a given invoice. Thin wrapper around
    /// [`Self::list`].
    pub async fn list_for_invoice(&self, invoice_id: &InvoiceId) -> Result<Vec<CreditNote>> {
        self.list(&CreditNoteFilter {
            invoice_id: Some(invoice_id.clone()),
            ..Default::default()
        })
        .await
    }
}

fn row_to_credit_note(row: &sqlx::any::AnyRow) -> Result<CreditNote> {
    let id_s: String = row.try_get("id")?;
    let id = id_s
        .parse::<CreditNoteId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let number: Option<String> = row.try_get("number")?;
    let invoice_id_s: String = row.try_get("invoice_id")?;
    let invoice_id = invoice_id_s
        .parse::<InvoiceId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let state_s: String = row.try_get("state")?;
    let state = cn_state_from_str(&state_s)?;
    let amount_s: String = row.try_get("amount")?;
    let currency_s: String = row.try_get("currency")?;
    let currency = currency_s
        .parse::<Currency>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let reason: Option<String> = row.try_get("reason")?;
    let refund_ref: Option<String> = row.try_get("refund_ref")?;
    let issued_at_s: Option<String> = row.try_get("issued_at")?;
    let issued_at = issued_at_s.as_deref().map(parse_ts).transpose()?;
    let created_at_s: String = row.try_get("created_at")?;
    let metadata_s: Option<String> = row.try_get("metadata")?;

    Ok(CreditNote {
        id,
        number,
        invoice_id,
        state,
        amount: parse_decimal(&amount_s)?,
        currency,
        reason,
        refund_ref,
        issued_at,
        created_at: parse_ts(&created_at_s)?,
        metadata: metadata_from_json(metadata_s.as_deref())?,
    })
}

pub(crate) fn cn_state_to_str(s: CreditNoteState) -> &'static str {
    match s {
        CreditNoteState::Draft => "draft",
        CreditNoteState::Issued => "issued",
    }
}

pub(crate) fn cn_state_from_str(s: &str) -> Result<CreditNoteState> {
    Ok(match s {
        "draft" => CreditNoteState::Draft,
        "issued" => CreditNoteState::Issued,
        other => {
            return Err(StoreError::InvalidValue {
                column: "credit_notes.state",
                value: other.to_string(),
            })
        }
    })
}

// ---------------------------------------------------------------------
// Credit-note state history
// ---------------------------------------------------------------------

/// Credit-note history repository (audit + outbox).
#[derive(Debug, Clone)]
pub struct CreditNoteHistoryRepo<'p> {
    pool: &'p Pool,
}

impl<'p> CreditNoteHistoryRepo<'p> {
    /// Construct.
    pub fn new(pool: &'p Pool) -> Self {
        Self { pool }
    }

    /// Append a row. Opens its own short transaction; for atomic
    /// composition with a credit-note mutation, use [`Self::save_in_tx`].
    pub async fn save(&self, h: &CreditNoteStateHistory) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::save_in_tx(&mut tx, h).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Append inside a caller-owned transaction.
    pub async fn save_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        h: &CreditNoteStateHistory,
    ) -> Result<()> {
        let metadata = metadata_to_json(&h.metadata)?;
        let sql = portable_sql_for_tx(
            tx,
            "INSERT INTO credit_note_state_history \
             (id, credit_note_id, from_state, to_state, event, actor, channel, bus_event_id, \
              occurred_at, published_at, metadata) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        );
        sqlx::query(&sql)
            .bind(h.id.to_string())
            .bind(h.credit_note_id.to_string())
            .bind(h.from_state.map(cn_state_to_str))
            .bind(cn_state_to_str(h.to_state))
            .bind(&h.event)
            .bind(h.actor.clone())
            .bind(super::history::channel_to_str(h.channel))
            .bind(h.bus_event_id.clone())
            .bind(ts_to_string(&h.occurred_at))
            .bind(h.published_at.as_ref().map(ts_to_string))
            .bind(metadata)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    /// List in order for a given credit note.
    pub async fn list_for_credit_note(
        &self,
        credit_note_id: &CreditNoteId,
    ) -> Result<Vec<CreditNoteStateHistory>> {
        let sql = portable_sql(
            self.pool,
            "SELECT id, credit_note_id, from_state, to_state, event, actor, channel, bus_event_id, \
                    occurred_at, published_at, metadata \
             FROM credit_note_state_history WHERE credit_note_id = ? ORDER BY occurred_at ASC",
        )
        .await?;
        let rows = sqlx::query(&sql)
            .bind(credit_note_id.to_string())
            .fetch_all(self.pool)
            .await?;
        rows.iter().map(row_to_cn_history).collect()
    }

    /// Outbox query: pending rows that need bus publication.
    pub async fn pending_outbox(&self, limit: i64) -> Result<Vec<CreditNoteStateHistory>> {
        let sql = portable_sql(
            self.pool,
            "SELECT id, credit_note_id, from_state, to_state, event, actor, channel, bus_event_id, \
                    occurred_at, published_at, metadata \
             FROM credit_note_state_history WHERE published_at IS NULL \
             ORDER BY occurred_at ASC LIMIT ?",
        )
        .await?;
        let rows = sqlx::query(&sql).bind(limit).fetch_all(self.pool).await?;
        rows.iter().map(row_to_cn_history).collect()
    }

    /// Mark an outbox row as published.
    pub async fn mark_published(&self, id: &HistoryId) -> Result<()> {
        let sql = portable_sql(
            self.pool,
            "UPDATE credit_note_state_history SET published_at = ? WHERE id = ?",
        )
        .await?;
        sqlx::query(&sql)
            .bind(ts_to_string(&Utc::now()))
            .bind(id.to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

fn row_to_cn_history(row: &sqlx::any::AnyRow) -> Result<CreditNoteStateHistory> {
    let id_s: String = row.try_get("id")?;
    let id = id_s
        .parse::<HistoryId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let credit_note_id_s: String = row.try_get("credit_note_id")?;
    let credit_note_id = credit_note_id_s
        .parse::<CreditNoteId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let from_state_s: Option<String> = row.try_get("from_state")?;
    let from_state = from_state_s.as_deref().map(cn_state_from_str).transpose()?;
    let to_state_s: String = row.try_get("to_state")?;
    let to_state = cn_state_from_str(&to_state_s)?;
    let event: String = row.try_get("event")?;
    let actor: Option<String> = row.try_get("actor")?;
    let channel_s: String = row.try_get("channel")?;
    let channel: HistoryChannel = super::history::channel_from_str(&channel_s)?;
    let bus_event_id: Option<String> = row.try_get("bus_event_id")?;
    let occurred_at_s: String = row.try_get("occurred_at")?;
    let published_at_s: Option<String> = row.try_get("published_at")?;
    let metadata_s: Option<String> = row.try_get("metadata")?;

    Ok(CreditNoteStateHistory {
        id,
        credit_note_id,
        from_state,
        to_state,
        event,
        actor,
        channel,
        bus_event_id,
        occurred_at: parse_ts(&occurred_at_s)?,
        published_at: published_at_s.as_deref().map(parse_ts).transpose()?,
        metadata: metadata_from_json(metadata_s.as_deref())?,
    })
}
