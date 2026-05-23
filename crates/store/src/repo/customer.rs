//! Customer repository.

use sqlx::Row;

use hop_top_inv_core::domain::address::Address;
use hop_top_inv_core::domain::customer::Customer;
use hop_top_inv_core::domain::ids::CustomerId;

use super::{metadata_from_json, metadata_to_json, parse_ts, ts_to_string};
use crate::error::{Result, StoreError};
use crate::pool::Pool;
use crate::sql::{portable_sql, portable_sql_for_tx};

/// Customer repository.
#[derive(Debug, Clone)]
pub struct CustomerRepo<'p> {
    pool: &'p Pool,
}

impl<'p> CustomerRepo<'p> {
    /// Construct against an open pool.
    pub fn new(pool: &'p Pool) -> Self {
        Self { pool }
    }

    /// Upsert. Uses `INSERT ... ON CONFLICT DO UPDATE` so existing
    /// rows are updated in place. (The prior DELETE+INSERT would have
    /// hit a FK RESTRICT violation if any invoices or schedules
    /// referenced this customer.)
    pub async fn save(&self, c: &Customer) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::save_in_tx(&mut tx, c).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Upsert inside a caller-owned transaction (see [`InvoiceRepo::save_in_tx`]).
    pub async fn save_in_tx(tx: &mut sqlx::Transaction<'_, sqlx::Any>, c: &Customer) -> Result<()> {
        let address_json = serde_json::to_string(&c.address)?;
        let metadata = metadata_to_json(&c.metadata)?;
        let sql = portable_sql_for_tx(
            tx,
            "INSERT INTO customers \
             (id, display_name, email, address_json, jurisdiction, metadata, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (id) DO UPDATE SET \
              display_name = excluded.display_name, \
              email = excluded.email, \
              address_json = excluded.address_json, \
              jurisdiction = excluded.jurisdiction, \
              metadata = excluded.metadata, \
              updated_at = excluded.updated_at",
        );
        sqlx::query(&sql)
            .bind(c.id.to_string())
            .bind(&c.display_name)
            .bind(c.email.clone())
            .bind(address_json)
            .bind(None::<String>) // jurisdiction is inferred from address; column reserved.
            .bind(metadata)
            .bind(ts_to_string(&c.created_at))
            .bind(ts_to_string(&c.updated_at))
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    /// Fetch by id.
    pub async fn get(&self, id: &CustomerId) -> Result<Option<Customer>> {
        let sql = portable_sql(
            self.pool,
            "SELECT id, display_name, email, address_json, metadata, created_at, updated_at \
             FROM customers WHERE id = ?",
        )
        .await?;
        let row = sqlx::query(&sql)
            .bind(id.to_string())
            .fetch_optional(self.pool)
            .await?;

        match row {
            None => Ok(None),
            Some(row) => Ok(Some(row_to_customer(&row)?)),
        }
    }

    /// List all customers (basic pagination via SQL LIMIT/OFFSET).
    pub async fn list(&self, limit: i64, offset: i64) -> Result<Vec<Customer>> {
        let sql = portable_sql(
            self.pool,
            "SELECT id, display_name, email, address_json, metadata, created_at, updated_at \
             FROM customers ORDER BY created_at ASC LIMIT ? OFFSET ?",
        )
        .await?;
        let rows = sqlx::query(&sql)
            .bind(limit)
            .bind(offset)
            .fetch_all(self.pool)
            .await?;

        rows.iter().map(row_to_customer).collect()
    }
}

fn row_to_customer(row: &sqlx::any::AnyRow) -> Result<Customer> {
    let id_s: String = row.try_get("id")?;
    let id = id_s
        .parse::<CustomerId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let display_name: String = row.try_get("display_name")?;
    let email: Option<String> = row.try_get("email")?;
    let address_json: String = row.try_get("address_json")?;
    let address: Address = serde_json::from_str(&address_json)?;
    let metadata_s: Option<String> = row.try_get("metadata")?;
    let metadata = metadata_from_json(metadata_s.as_deref())?;
    let created_at_s: String = row.try_get("created_at")?;
    let updated_at_s: String = row.try_get("updated_at")?;

    Ok(Customer {
        id,
        display_name,
        email,
        address,
        metadata,
        created_at: parse_ts(&created_at_s)?,
        updated_at: parse_ts(&updated_at_s)?,
    })
}
