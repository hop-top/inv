//! Invoice + invoice-line repositories.

use sqlx::Row;

use hop_top_inv_core::domain::ids::{CustomerId, InvoiceId, LineId, ScheduleId};
use hop_top_inv_core::domain::invoice::{Invoice, InvoiceLine, InvoiceState, TaxCategory};
use hop_top_inv_core::domain::jurisdiction::Jurisdiction;
use hop_top_inv_core::domain::money::Currency;

use super::{
    decimal_to_string, metadata_from_json, metadata_to_json, parse_decimal, parse_ts, ts_to_string,
};
use crate::error::{Result, StoreError};
use crate::pool::Pool;
use crate::sql::{portable_sql, portable_sql_for_tx};

// ---------------------------------------------------------------------
// Invoice
// ---------------------------------------------------------------------

/// Filter for listing invoices.
#[derive(Debug, Default, Clone)]
pub struct InvoiceFilter {
    /// Restrict to a specific customer.
    pub customer_id: Option<CustomerId>,
    /// Restrict to a specific lifecycle state.
    pub state: Option<InvoiceState>,
    /// Max rows.
    pub limit: Option<i64>,
    /// Offset for paging.
    pub offset: Option<i64>,
}

/// Invoice repository.
#[derive(Debug, Clone)]
pub struct InvoiceRepo<'p> {
    pool: &'p Pool,
}

impl<'p> InvoiceRepo<'p> {
    /// Construct against an open pool.
    pub fn new(pool: &'p Pool) -> Self {
        Self { pool }
    }

    /// Upsert. Inserts a new row or updates an existing one in place
    /// without triggering `ON DELETE CASCADE` on related history rows.
    ///
    /// Opens its own short transaction. For multi-step ops (e.g. FSM
    /// transition + history-row insert) where the caller needs both
    /// writes to commit atomically, use [`Self::save_in_tx`] instead.
    pub async fn save(&self, inv: &Invoice) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::save_in_tx(&mut tx, inv).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Upsert inside a caller-owned transaction.
    ///
    /// Use this when the invoice mutation must commit atomically with
    /// other writes (history-row insert, line replacement, outbox
    /// publish). See design §3.5 — every command runs FSM mutation +
    /// `invoice_state_history` insert in one transaction.
    pub async fn save_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        inv: &Invoice,
    ) -> Result<()> {
        let metadata = metadata_to_json(&inv.metadata)?;
        let sql = portable_sql_for_tx(
            tx,
            "INSERT INTO invoices \
             (id, number, customer_id, seller_jur, currency, state, \
              issued_at, due_at, sent_at, viewed_at, paid_at, voided_at, \
              subtotal, tax_total, total, amount_paid, \
              schedule_id, template_path, pdf_blob_ref, idempotency_key, \
              nexus_review, metadata, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (id) DO UPDATE SET \
              number = excluded.number, \
              customer_id = excluded.customer_id, \
              seller_jur = excluded.seller_jur, \
              currency = excluded.currency, \
              state = excluded.state, \
              issued_at = excluded.issued_at, \
              due_at = excluded.due_at, \
              sent_at = excluded.sent_at, \
              viewed_at = excluded.viewed_at, \
              paid_at = excluded.paid_at, \
              voided_at = excluded.voided_at, \
              subtotal = excluded.subtotal, \
              tax_total = excluded.tax_total, \
              total = excluded.total, \
              amount_paid = excluded.amount_paid, \
              schedule_id = excluded.schedule_id, \
              template_path = excluded.template_path, \
              pdf_blob_ref = excluded.pdf_blob_ref, \
              idempotency_key = excluded.idempotency_key, \
              nexus_review = excluded.nexus_review, \
              metadata = excluded.metadata, \
              updated_at = excluded.updated_at",
        );
        sqlx::query(&sql)
            .bind(inv.id.to_string())
            .bind(inv.number.clone())
            .bind(inv.customer_id.to_string())
            .bind(inv.seller_jurisdiction.as_str())
            .bind(inv.currency.to_string())
            .bind(state_to_str(inv.state))
            .bind(inv.issued_at.as_ref().map(ts_to_string))
            .bind(inv.due_at.as_ref().map(ts_to_string))
            .bind(inv.sent_at.as_ref().map(ts_to_string))
            .bind(inv.viewed_at.as_ref().map(ts_to_string))
            .bind(inv.paid_at.as_ref().map(ts_to_string))
            .bind(inv.voided_at.as_ref().map(ts_to_string))
            .bind(decimal_to_string(&inv.subtotal))
            .bind(decimal_to_string(&inv.tax_total))
            .bind(decimal_to_string(&inv.total))
            .bind(decimal_to_string(&inv.amount_paid))
            .bind(inv.schedule_id.as_ref().map(|s| s.to_string()))
            .bind(inv.template_path.clone())
            .bind(inv.pdf_blob_ref.clone())
            .bind(inv.idempotency_key.clone())
            .bind(i64::from(inv.nexus_review))
            .bind(metadata)
            .bind(ts_to_string(&inv.created_at))
            .bind(ts_to_string(&inv.updated_at))
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    /// Fetch by id.
    pub async fn get(&self, id: &InvoiceId) -> Result<Option<Invoice>> {
        let sql = portable_sql(self.pool, &invoice_select_columns()).await?;
        let row = sqlx::query(&sql)
            .bind(id.to_string())
            .fetch_optional(self.pool)
            .await?;
        row.as_ref().map(row_to_invoice).transpose()
    }

    /// Indexed lookup by `idempotency_key`.
    ///
    /// Used by command-layer dedupe (e.g. `draft_invoice`'s
    /// idempotency-replay path). The column has a UNIQUE constraint at
    /// schema level so the query is O(1) — `LIMIT 1` is belt-and-braces.
    pub async fn find_by_idempotency_key(&self, key: &str) -> Result<Option<Invoice>> {
        let sql = portable_sql(
            self.pool,
            "SELECT id, number, customer_id, seller_jur, currency, state, \
                    issued_at, due_at, sent_at, viewed_at, paid_at, voided_at, \
                    subtotal, tax_total, total, amount_paid, \
                    schedule_id, template_path, pdf_blob_ref, idempotency_key, \
                    nexus_review, metadata, created_at, updated_at \
             FROM invoices WHERE idempotency_key = ? LIMIT 1",
        )
        .await?;
        let row = sqlx::query(&sql)
            .bind(key)
            .fetch_optional(self.pool)
            .await?;
        row.as_ref().map(row_to_invoice).transpose()
    }

    /// List with filters.
    pub async fn list(&self, filter: &InvoiceFilter) -> Result<Vec<Invoice>> {
        // Build the WHERE dynamically. The SQL is assembled in sqlite's
        // `?` form, then [`portable_sql`] rewrites to `$N` for postgres
        // immediately before query() consumes it.
        let mut sql = String::from(
            "SELECT id, number, customer_id, seller_jur, currency, state, \
                    issued_at, due_at, sent_at, viewed_at, paid_at, voided_at, \
                    subtotal, tax_total, total, amount_paid, \
                    schedule_id, template_path, pdf_blob_ref, idempotency_key, \
                    nexus_review, metadata, created_at, updated_at \
             FROM invoices WHERE 1=1",
        );
        if filter.customer_id.is_some() {
            sql.push_str(" AND customer_id = ?");
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
        if let Some(c) = filter.customer_id.as_ref() {
            q = q.bind(c.to_string());
        }
        if let Some(s) = filter.state {
            q = q.bind(state_to_str(s));
        }
        if let Some(l) = filter.limit {
            q = q.bind(l);
        }
        if let Some(o) = filter.offset {
            q = q.bind(o);
        }
        let rows = q.fetch_all(self.pool).await?;
        rows.iter().map(row_to_invoice).collect()
    }

    /// Delete (cascades through invoice_lines, history, etc).
    pub async fn delete(&self, id: &InvoiceId) -> Result<()> {
        let sql = portable_sql(self.pool, "DELETE FROM invoices WHERE id = ?").await?;
        sqlx::query(&sql)
            .bind(id.to_string())
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

fn invoice_select_columns() -> String {
    String::from(
        "SELECT id, number, customer_id, seller_jur, currency, state, \
                issued_at, due_at, sent_at, viewed_at, paid_at, voided_at, \
                subtotal, tax_total, total, amount_paid, \
                schedule_id, template_path, pdf_blob_ref, idempotency_key, \
                nexus_review, metadata, created_at, updated_at \
         FROM invoices WHERE id = ?",
    )
}

fn row_to_invoice(row: &sqlx::any::AnyRow) -> Result<Invoice> {
    let id_s: String = row.try_get("id")?;
    let id = id_s
        .parse::<InvoiceId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let number: Option<String> = row.try_get("number")?;
    let customer_id_s: String = row.try_get("customer_id")?;
    let customer_id = customer_id_s
        .parse::<CustomerId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let seller_jur_s: String = row.try_get("seller_jur")?;
    let seller_jurisdiction = seller_jur_s
        .parse::<Jurisdiction>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let currency_s: String = row.try_get("currency")?;
    let currency = currency_s
        .parse::<Currency>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let state_s: String = row.try_get("state")?;
    let state = state_from_str(&state_s)?;

    let issued_at = opt_ts(row, "issued_at")?;
    let due_at = opt_ts(row, "due_at")?;
    let sent_at = opt_ts(row, "sent_at")?;
    let viewed_at = opt_ts(row, "viewed_at")?;
    let paid_at = opt_ts(row, "paid_at")?;
    let voided_at = opt_ts(row, "voided_at")?;

    let subtotal_s: String = row.try_get("subtotal")?;
    let tax_total_s: String = row.try_get("tax_total")?;
    let total_s: String = row.try_get("total")?;
    let amount_paid_s: String = row.try_get("amount_paid")?;

    let schedule_id_s: Option<String> = row.try_get("schedule_id")?;
    let schedule_id = match schedule_id_s {
        None => None,
        Some(s) => Some(
            s.parse::<ScheduleId>()
                .map_err(|e| StoreError::Id(e.to_string()))?,
        ),
    };
    let template_path: Option<String> = row.try_get("template_path")?;
    let pdf_blob_ref: Option<String> = row.try_get("pdf_blob_ref")?;
    let idempotency_key: Option<String> = row.try_get("idempotency_key")?;
    let nexus_review: i64 = row.try_get("nexus_review")?;
    let nexus_review = nexus_review != 0;
    let metadata_s: Option<String> = row.try_get("metadata")?;
    let metadata = metadata_from_json(metadata_s.as_deref())?;
    let created_at_s: String = row.try_get("created_at")?;
    let updated_at_s: String = row.try_get("updated_at")?;

    Ok(Invoice {
        id,
        number,
        customer_id,
        seller_jurisdiction,
        currency,
        state,
        issued_at,
        due_at,
        sent_at,
        viewed_at,
        paid_at,
        voided_at,
        subtotal: parse_decimal(&subtotal_s)?,
        tax_total: parse_decimal(&tax_total_s)?,
        total: parse_decimal(&total_s)?,
        amount_paid: parse_decimal(&amount_paid_s)?,
        schedule_id,
        template_path,
        pdf_blob_ref,
        idempotency_key,
        nexus_review,
        metadata,
        created_at: parse_ts(&created_at_s)?,
        updated_at: parse_ts(&updated_at_s)?,
    })
}

fn opt_ts(row: &sqlx::any::AnyRow, col: &str) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    let s: Option<String> = row.try_get(col)?;
    match s {
        None => Ok(None),
        Some(text) => parse_ts(&text).map(Some),
    }
}

pub(crate) fn state_to_str(s: InvoiceState) -> &'static str {
    match s {
        InvoiceState::Draft => "draft",
        InvoiceState::Issued => "issued",
        InvoiceState::Sent => "sent",
        InvoiceState::Viewed => "viewed",
        InvoiceState::PartiallyPaid => "partially_paid",
        InvoiceState::Paid => "paid",
        InvoiceState::Voided => "voided",
    }
}

pub(crate) fn state_from_str(s: &str) -> Result<InvoiceState> {
    Ok(match s {
        "draft" => InvoiceState::Draft,
        "issued" => InvoiceState::Issued,
        "sent" => InvoiceState::Sent,
        "viewed" => InvoiceState::Viewed,
        "partially_paid" => InvoiceState::PartiallyPaid,
        "paid" => InvoiceState::Paid,
        "voided" => InvoiceState::Voided,
        other => {
            return Err(StoreError::InvalidValue {
                column: "invoices.state",
                value: other.to_string(),
            })
        }
    })
}

// ---------------------------------------------------------------------
// Invoice lines
// ---------------------------------------------------------------------

/// Invoice-line repository.
#[derive(Debug, Clone)]
pub struct InvoiceLineRepo<'p> {
    pool: &'p Pool,
}

impl<'p> InvoiceLineRepo<'p> {
    /// Construct against an open pool.
    pub fn new(pool: &'p Pool) -> Self {
        Self { pool }
    }

    /// Upsert a single line.
    pub async fn save(&self, line: &InvoiceLine) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::save_in_tx(&mut tx, line).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Upsert inside a caller-owned transaction.
    pub async fn save_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        line: &InvoiceLine,
    ) -> Result<()> {
        let tax_rate_ids = serde_json::to_string(&line.tax_rate_ids)?;
        let metadata = metadata_to_json(&line.metadata)?;
        let sql = portable_sql_for_tx(
            tx,
            "INSERT INTO invoice_lines \
             (id, invoice_id, position, description, quantity, unit_price, \
              tax_rate_ids, tax_category, tax_amount, line_total, metadata) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (id) DO UPDATE SET \
              invoice_id = excluded.invoice_id, \
              position = excluded.position, \
              description = excluded.description, \
              quantity = excluded.quantity, \
              unit_price = excluded.unit_price, \
              tax_rate_ids = excluded.tax_rate_ids, \
              tax_category = excluded.tax_category, \
              tax_amount = excluded.tax_amount, \
              line_total = excluded.line_total, \
              metadata = excluded.metadata",
        );
        sqlx::query(&sql)
            .bind(line.id.to_string())
            .bind(line.invoice_id.to_string())
            .bind(line.position as i64)
            .bind(&line.description)
            .bind(decimal_to_string(&line.quantity))
            .bind(decimal_to_string(&line.unit_price))
            .bind(tax_rate_ids)
            .bind(tax_category_to_str(line.tax_category))
            .bind(decimal_to_string(&line.tax_amount))
            .bind(decimal_to_string(&line.line_total))
            .bind(metadata)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    /// Replace all lines for an invoice atomically.
    ///
    /// Unlike the other repo methods, this one intentionally
    /// DELETEs-then-INSERTs because it semantically replaces the *set*
    /// of lines (e.g. line removed by a draft edit). No CASCADEing
    /// children hang off invoice_lines, so the DELETE is safe.
    pub async fn replace_for_invoice(
        &self,
        invoice_id: &InvoiceId,
        lines: &[InvoiceLine],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::replace_for_invoice_in_tx(&mut tx, invoice_id, lines).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Replace inside a caller-owned transaction.
    pub async fn replace_for_invoice_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Any>,
        invoice_id: &InvoiceId,
        lines: &[InvoiceLine],
    ) -> Result<()> {
        let delete_sql = portable_sql_for_tx(tx, "DELETE FROM invoice_lines WHERE invoice_id = ?");
        sqlx::query(&delete_sql)
            .bind(invoice_id.to_string())
            .execute(&mut **tx)
            .await?;
        for line in lines {
            let tax_rate_ids = serde_json::to_string(&line.tax_rate_ids)?;
            let metadata = metadata_to_json(&line.metadata)?;
            let insert_sql = portable_sql_for_tx(
                tx,
                "INSERT INTO invoice_lines \
                 (id, invoice_id, position, description, quantity, unit_price, \
                  tax_rate_ids, tax_category, tax_amount, line_total, metadata) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            );
            sqlx::query(&insert_sql)
                .bind(line.id.to_string())
                .bind(line.invoice_id.to_string())
                .bind(line.position as i64)
                .bind(&line.description)
                .bind(decimal_to_string(&line.quantity))
                .bind(decimal_to_string(&line.unit_price))
                .bind(tax_rate_ids)
                .bind(tax_category_to_str(line.tax_category))
                .bind(decimal_to_string(&line.tax_amount))
                .bind(decimal_to_string(&line.line_total))
                .bind(metadata)
                .execute(&mut **tx)
                .await?;
        }
        Ok(())
    }

    /// Fetch all lines for an invoice, sorted by `position`.
    pub async fn list_for_invoice(&self, invoice_id: &InvoiceId) -> Result<Vec<InvoiceLine>> {
        let sql = portable_sql(
            self.pool,
            "SELECT id, invoice_id, position, description, quantity, unit_price, \
                    tax_rate_ids, tax_category, tax_amount, line_total, metadata \
             FROM invoice_lines WHERE invoice_id = ? ORDER BY position ASC",
        )
        .await?;
        let rows = sqlx::query(&sql)
            .bind(invoice_id.to_string())
            .fetch_all(self.pool)
            .await?;

        rows.iter().map(row_to_invoice_line).collect()
    }
}

fn row_to_invoice_line(row: &sqlx::any::AnyRow) -> Result<InvoiceLine> {
    let id_s: String = row.try_get("id")?;
    let id = id_s
        .parse::<LineId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let invoice_id_s: String = row.try_get("invoice_id")?;
    let invoice_id = invoice_id_s
        .parse::<InvoiceId>()
        .map_err(|e| StoreError::Id(e.to_string()))?;
    let position: i64 = row.try_get("position")?;
    let description: String = row.try_get("description")?;
    let quantity_s: String = row.try_get("quantity")?;
    let unit_price_s: String = row.try_get("unit_price")?;
    let tax_rate_ids_s: Option<String> = row.try_get("tax_rate_ids")?;
    let tax_rate_ids: Vec<String> = match tax_rate_ids_s.as_deref() {
        None | Some("") => Vec::new(),
        Some(json) => serde_json::from_str(json)?,
    };
    let tax_category_s: String = row.try_get("tax_category")?;
    let tax_category = tax_category_from_str(&tax_category_s)?;
    let tax_amount_s: String = row.try_get("tax_amount")?;
    let line_total_s: String = row.try_get("line_total")?;
    let metadata_s: Option<String> = row.try_get("metadata")?;

    Ok(InvoiceLine {
        id,
        invoice_id,
        position: position as u32,
        description,
        quantity: parse_decimal(&quantity_s)?,
        unit_price: parse_decimal(&unit_price_s)?,
        tax_rate_ids,
        tax_category,
        tax_amount: parse_decimal(&tax_amount_s)?,
        line_total: parse_decimal(&line_total_s)?,
        metadata: metadata_from_json(metadata_s.as_deref())?,
    })
}

fn tax_category_to_str(c: TaxCategory) -> &'static str {
    match c {
        TaxCategory::Standard => "standard",
        TaxCategory::Reduced => "reduced",
        TaxCategory::ZeroRated => "zero_rated",
        TaxCategory::Exempt => "exempt",
    }
}

fn tax_category_from_str(s: &str) -> Result<TaxCategory> {
    Ok(match s {
        "standard" => TaxCategory::Standard,
        "reduced" => TaxCategory::Reduced,
        "zero_rated" => TaxCategory::ZeroRated,
        "exempt" => TaxCategory::Exempt,
        other => {
            return Err(StoreError::InvalidValue {
                column: "invoice_lines.tax_category",
                value: other.to_string(),
            })
        }
    })
}
