//! SQL placeholder rewriting for cross-backend query compatibility.
//!
//! Every query in the repository layer is hand-written with sqlite-style
//! `?` placeholders. PostgreSQL rejects `?` and demands numbered `$N`
//! parameters; MySQL / TiDB accept `?` like sqlite. `sqlx::Any` does
//! *not* perform this rewrite on our behalf (verified empirically against
//! sqlx 0.8.6 — `?`-laden statements come back with `42601` syntax errors
//! on postgres).
//!
//! Helpers here let repository code keep the source SQL strings unchanged
//! while still working on every supported backend:
//!
//! ```ignore
//! sqlx::query(&portable_sql_for_tx(&mut tx, "INSERT ... VALUES (?, ?)"))
//!     .bind(a)
//!     .bind(b)
//!     .execute(&mut *tx)
//!     .await?;
//! ```
//!
//! The rewriter is a naive whole-string scan that does NOT skip `?`
//! embedded in string literals. No inv query embeds a literal `?` today
//! (grep confirms); if that changes the helper must grow to a real
//! tokenizer. Placeholder numbering follows positional order — the first
//! `?` becomes `$1`, the second `$2`, etc. — which matches the order
//! sqlx's `bind()` chain sends parameters.
//!
//! Caching: detecting the backend on a `Pool` requires an async
//! connection acquire (see [`crate::pool::backend_of`]). [`portable_sql`]
//! pays that cost on every call; profile and add caching if it shows up
//! hot. The `_for_tx` form is cheap — the transaction already owns a
//! connection, so the backend is read via `Deref` with no async work.

use crate::error::Result;
use crate::pool::{backend_of, Backend, Pool};

/// Rewrite a sqlite-flavoured SQL string to whatever placeholder syntax
/// the connected backend speaks, by inspecting the pool.
///
/// Returns the original string for sqlite/MySQL; converts `?` → `$1, $2,
/// …` for postgres.
pub async fn portable_sql(pool: &Pool, sql: &str) -> Result<String> {
    Ok(rewrite(backend_of(pool).await?, sql))
}

/// Synchronous variant for use inside a `sqlx::Transaction`. The
/// transaction owns a live connection, so its backend can be read
/// without an extra acquire.
pub fn portable_sql_for_tx(tx: &sqlx::Transaction<'_, sqlx::Any>, sql: &str) -> String {
    // `Transaction<'_, Any>` derefs to `AnyConnection`, which exposes
    // `backend_name()`. Map to our `Backend` enum.
    let name = tx.backend_name().to_ascii_lowercase();
    let backend = match name.as_str() {
        "postgresql" | "postgres" => Backend::Postgres,
        "mysql" => Backend::Mysql,
        // Default to sqlite (no rewrite). Any unknown backend falls
        // through here; the caller's query will then likely fail at
        // execute() time with a more informative error.
        _ => Backend::Sqlite,
    };
    rewrite(backend, sql)
}

/// Pure placeholder rewrite. Postgres → numbered; others → unchanged.
pub fn rewrite(backend: Backend, sql: &str) -> String {
    match backend {
        Backend::Postgres => rewrite_postgres(sql),
        Backend::Sqlite | Backend::Mysql => sql.to_string(),
    }
}

fn rewrite_postgres(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len() + 8);
    let mut n: usize = 0;
    for ch in sql.chars() {
        if ch == '?' {
            n += 1;
            out.push('$');
            // Inline-format the index; avoids pulling fmt::Write.
            out.push_str(&n.to_string());
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_postgres_zero_placeholders() {
        let sql = "SELECT * FROM customers";
        assert_eq!(rewrite(Backend::Postgres, sql), sql);
    }

    #[test]
    fn rewrite_postgres_single_placeholder() {
        let sql = "SELECT * FROM customers WHERE id = ?";
        assert_eq!(
            rewrite(Backend::Postgres, sql),
            "SELECT * FROM customers WHERE id = $1"
        );
    }

    #[test]
    fn rewrite_postgres_multiple_placeholders() {
        let sql = "INSERT INTO customers (id, name) VALUES (?, ?)";
        assert_eq!(
            rewrite(Backend::Postgres, sql),
            "INSERT INTO customers (id, name) VALUES ($1, $2)"
        );
    }

    #[test]
    fn rewrite_postgres_many_placeholders_numbered_in_order() {
        let sql = "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
        assert_eq!(
            rewrite(Backend::Postgres, sql),
            "VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"
        );
    }

    #[test]
    fn rewrite_postgres_preserves_other_punctuation_and_newlines() {
        let sql = "INSERT INTO t (a, b)\n  VALUES (?, ?)\n  ON CONFLICT (a) DO NOTHING";
        let expected = "INSERT INTO t (a, b)\n  VALUES ($1, $2)\n  ON CONFLICT (a) DO NOTHING";
        assert_eq!(rewrite(Backend::Postgres, sql), expected);
    }

    #[test]
    fn rewrite_postgres_handles_more_than_nine_params() {
        // Boundary: $10 must follow $9 as two characters.
        let sql = "VALUES (?,?,?,?,?,?,?,?,?,?)";
        assert_eq!(
            rewrite(Backend::Postgres, sql),
            "VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)"
        );
    }

    #[test]
    fn rewrite_sqlite_unchanged() {
        let sql = "INSERT INTO customers (id, name) VALUES (?, ?)";
        assert_eq!(rewrite(Backend::Sqlite, sql), sql);
    }

    #[test]
    fn rewrite_mysql_unchanged() {
        let sql = "INSERT INTO customers (id, name) VALUES (?, ?)";
        assert_eq!(rewrite(Backend::Mysql, sql), sql);
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn portable_sql_on_sqlite_pool_is_identity() {
        let pool = crate::pool::connect("sqlite::memory:").await.unwrap();
        let sql = "SELECT ?";
        assert_eq!(portable_sql(&pool, sql).await.unwrap(), sql);
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn portable_sql_for_tx_on_sqlite_is_identity() {
        let pool = crate::pool::connect("sqlite::memory:").await.unwrap();
        let tx = pool.begin().await.unwrap();
        let sql = "SELECT ?, ?";
        assert_eq!(portable_sql_for_tx(&tx, sql), sql);
    }
}
