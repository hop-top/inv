//! Schedule repository.

use sqlx::Row;
use std::str::FromStr;

use inv_core::domain::ids::{CustomerId, ScheduleId};
use inv_core::domain::money::Currency;
use inv_core::domain::schedule::{Cadence, Schedule, ScheduleLine, ScheduleState};

use super::{metadata_from_json, metadata_to_json, parse_ts, ts_to_string};
use crate::error::{Result, StoreError};
use crate::pool::Pool;

/// Schedule repository.
#[derive(Debug, Clone)]
pub struct ScheduleRepo<'p> {
    pool: &'p Pool,
}

impl<'p> ScheduleRepo<'p> {
    /// Construct.
    pub fn new(pool: &'p Pool) -> Self {
        Self { pool }
    }

    /// Upsert. Uses `INSERT ... ON CONFLICT DO UPDATE`.
    pub async fn save(&self, s: &Schedule) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::save_in_tx(&mut tx, s).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Upsert inside a caller-owned transaction (see [`InvoiceRepo::save_in_tx`]).
    pub async fn save_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        s: &Schedule,
    ) -> Result<()> {
        let metadata = metadata_to_json(&s.metadata)?;
        let template_lines = serde_json::to_string(&s.template_lines)?;
        sqlx::query(
            "INSERT INTO schedules \
             (id, customer_id, template_lines, currency, cadence, start_date, end_date, \
              auto_issue, next_run, last_run, state, metadata, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (id) DO UPDATE SET \
              customer_id = excluded.customer_id, \
              template_lines = excluded.template_lines, \
              currency = excluded.currency, \
              cadence = excluded.cadence, \
              start_date = excluded.start_date, \
              end_date = excluded.end_date, \
              auto_issue = excluded.auto_issue, \
              next_run = excluded.next_run, \
              last_run = excluded.last_run, \
              state = excluded.state, \
              metadata = excluded.metadata, \
              updated_at = excluded.updated_at",
        )
        .bind(s.id.to_string())
        .bind(s.customer_id.to_string())
        .bind(template_lines)
        .bind(s.currency.to_string())
        .bind(s.cadence.to_string())
        .bind(s.start_date.to_string())
        .bind(s.end_date.as_ref().map(|d| d.to_string()))
        .bind(i64::from(s.auto_issue))
        .bind(s.next_run.to_string())
        .bind(s.last_run.as_ref().map(|d| d.to_string()))
        .bind(state_to_str(s.state))
        .bind(metadata)
        .bind(ts_to_string(&s.created_at))
        .bind(ts_to_string(&s.updated_at))
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// Get by id.
    pub async fn get(&self, id: &ScheduleId) -> Result<Option<Schedule>> {
        let row = sqlx::query(
            "SELECT id, customer_id, template_lines, currency, cadence, start_date, end_date, \
                    auto_issue, next_run, last_run, state, metadata, created_at, updated_at \
             FROM schedules WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_optional(self.pool)
        .await?;
        row.as_ref().map(row_to_schedule).transpose()
    }

    /// List schedules for a customer.
    pub async fn list_for_customer(&self, customer_id: &CustomerId) -> Result<Vec<Schedule>> {
        let rows = sqlx::query(
            "SELECT id, customer_id, template_lines, currency, cadence, start_date, end_date, \
                    auto_issue, next_run, last_run, state, metadata, created_at, updated_at \
             FROM schedules WHERE customer_id = ? ORDER BY created_at ASC",
        )
        .bind(customer_id.to_string())
        .fetch_all(self.pool)
        .await?;
        rows.iter().map(row_to_schedule).collect()
    }

    /// Active schedules whose `next_run` is on or before `cutoff` (a date
    /// string in `YYYY-MM-DD` form). Useful for the ticker.
    pub async fn due(&self, cutoff: chrono::NaiveDate) -> Result<Vec<Schedule>> {
        let rows = sqlx::query(
            "SELECT id, customer_id, template_lines, currency, cadence, start_date, end_date, \
                    auto_issue, next_run, last_run, state, metadata, created_at, updated_at \
             FROM schedules WHERE state = 'active' AND next_run <= ? \
             ORDER BY next_run ASC",
        )
        .bind(cutoff.to_string())
        .fetch_all(self.pool)
        .await?;
        rows.iter().map(row_to_schedule).collect()
    }
}

fn row_to_schedule(row: &sqlx::any::AnyRow) -> Result<Schedule> {
    let id_s: String = row.try_get("id")?;
    let id = id_s
        .parse::<ScheduleId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let customer_id_s: String = row.try_get("customer_id")?;
    let customer_id = customer_id_s
        .parse::<CustomerId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let template_lines_s: String = row.try_get("template_lines")?;
    let template_lines: Vec<ScheduleLine> = serde_json::from_str(&template_lines_s)?;
    let currency_s: String = row.try_get("currency")?;
    let currency = currency_s
        .parse::<Currency>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let cadence_s: String = row.try_get("cadence")?;
    let cadence = Cadence::from_str(&cadence_s).map_err(|e| StoreError::Id(e.to_string()))?;
    let start_date_s: String = row.try_get("start_date")?;
    let start_date = chrono::NaiveDate::from_str(&start_date_s)
        .map_err(|e| StoreError::Other(format!("start_date parse: {e}")))?;
    let end_date_s: Option<String> = row.try_get("end_date")?;
    let end_date = end_date_s
        .as_deref()
        .map(chrono::NaiveDate::from_str)
        .transpose()
        .map_err(|e| StoreError::Other(format!("end_date parse: {e}")))?;
    let auto_issue: i64 = row.try_get("auto_issue")?;
    let auto_issue = auto_issue != 0;
    let next_run_s: String = row.try_get("next_run")?;
    let next_run = chrono::NaiveDate::from_str(&next_run_s)
        .map_err(|e| StoreError::Other(format!("next_run parse: {e}")))?;
    let last_run_s: Option<String> = row.try_get("last_run")?;
    let last_run = last_run_s
        .as_deref()
        .map(chrono::NaiveDate::from_str)
        .transpose()
        .map_err(|e| StoreError::Other(format!("last_run parse: {e}")))?;
    let state_s: String = row.try_get("state")?;
    let state = state_from_str(&state_s)?;
    let metadata_s: Option<String> = row.try_get("metadata")?;
    let created_at_s: String = row.try_get("created_at")?;
    let updated_at_s: String = row.try_get("updated_at")?;

    Ok(Schedule {
        id,
        customer_id,
        template_lines,
        currency,
        cadence,
        start_date,
        end_date,
        auto_issue,
        next_run,
        last_run,
        state,
        metadata: metadata_from_json(metadata_s.as_deref())?,
        created_at: parse_ts(&created_at_s)?,
        updated_at: parse_ts(&updated_at_s)?,
    })
}

fn state_to_str(s: ScheduleState) -> &'static str {
    match s {
        ScheduleState::Active => "active",
        ScheduleState::Paused => "paused",
        ScheduleState::Cancelled => "cancelled",
    }
}

fn state_from_str(s: &str) -> Result<ScheduleState> {
    Ok(match s {
        "active" => ScheduleState::Active,
        "paused" => ScheduleState::Paused,
        "cancelled" => ScheduleState::Cancelled,
        other => {
            return Err(StoreError::InvalidValue {
                column: "schedules.state",
                value: other.to_string(),
            })
        }
    })
}
