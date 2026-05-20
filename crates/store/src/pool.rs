//! Async DB pool builder.
//!
//! `sqlx::Any` lets us write a single repository-layer codepath across
//! sqlite/postgres/mysql. The trade-off is loss of compile-time query
//! checking (`query!` / `query_as!` need a concrete backend); the
//! repositories below use runtime `query()` with manual mapping.
//!
//! Backends are installed at runtime; the [`connect`] helper does the
//! installation once on first call and then opens an `AnyPool` against
//! the caller-supplied DSN. Backend identity is recovered later by
//! [`backend_of`], which acquires a short-lived connection from the pool
//! and reads `AnyConnection::backend_name()`. `sqlx 0.8` removed
//! `AnyPool::any_kind()`, so this is the supported path.

use std::sync::OnceLock;

use sqlx::any::{install_default_drivers, AnyPoolOptions};

use crate::error::{Result, StoreError};

/// Re-export the pool type — repositories take `&Pool` everywhere.
pub type Pool = sqlx::AnyPool;

/// Make sure sqlx's runtime driver registry is initialised exactly once.
fn ensure_drivers() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(install_default_drivers);
}

/// Open an async pool against the given DSN.
///
/// Recognised DSN forms (driver feature must be enabled):
/// - `sqlite::memory:` or `sqlite:./path.db` → `sqlite` feature
/// - `postgres://user:pw@host/db` → `postgres` feature
/// - `mysql://user:pw@host/db` → `tidb` feature
pub async fn connect(dsn: &str) -> Result<Pool> {
    ensure_drivers();

    // Pre-flight: emit a friendlier error than sqlx's "no driver" when
    // the user asks for a backend we weren't built with.
    let scheme = scheme_of(dsn);
    if backend_from_scheme(&scheme).is_none() {
        return Err(StoreError::UnsupportedDsn(dsn.to_string()));
    }

    // sqlite `:memory:` is per-connection — every fresh connection gets
    // its own empty database, which breaks pooling. For that case we
    // pin the pool to a single connection so writes + reads see the
    // same store. File-backed sqlite + other backends use the default.
    let max_conn = if is_sqlite_in_memory(dsn) { 1 } else { 5 };
    let pool = AnyPoolOptions::new()
        .max_connections(max_conn)
        .min_connections(if is_sqlite_in_memory(dsn) { 1 } else { 0 })
        .connect(dsn)
        .await?;
    Ok(pool)
}

fn is_sqlite_in_memory(dsn: &str) -> bool {
    let d = dsn.to_ascii_lowercase();
    d == "sqlite::memory:" || d.starts_with("sqlite::memory:?") || d.contains(":memory:")
}

/// Backend variants we know how to migrate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    /// SQLite (file or `:memory:`).
    Sqlite,
    /// PostgreSQL.
    Postgres,
    /// MySQL wire (TiDB; v1 alias).
    Mysql,
}

/// Detect the backend used by an open pool by reading
/// `AnyConnection::backend_name()` off a freshly acquired connection.
pub async fn backend_of(pool: &Pool) -> Result<Backend> {
    let conn = pool.acquire().await?;
    let name = conn.backend_name().to_ascii_lowercase();
    Backend::from_backend_name(&name).ok_or_else(|| StoreError::InvalidValue {
        column: "backend_name",
        value: name,
    })
}

impl Backend {
    /// Map `AnyConnection::backend_name()` (e.g. `"SQLite"`, `"PostgreSQL"`,
    /// `"MySQL"`) to a [`Backend`]. Case-insensitive; accepts plural and
    /// short forms.
    fn from_backend_name(name: &str) -> Option<Backend> {
        match name {
            "sqlite" => Some(Backend::Sqlite),
            "postgresql" | "postgres" => Some(Backend::Postgres),
            "mysql" => Some(Backend::Mysql),
            _ => None,
        }
    }
}

fn scheme_of(dsn: &str) -> String {
    let head = dsn.split_once(':').map(|(s, _)| s).unwrap_or(dsn);
    head.to_ascii_lowercase()
}

fn backend_from_scheme(scheme: &str) -> Option<Backend> {
    match scheme {
        #[cfg(feature = "sqlite")]
        "sqlite" => Some(Backend::Sqlite),
        #[cfg(feature = "postgres")]
        "postgres" | "postgresql" => Some(Backend::Postgres),
        #[cfg(feature = "tidb")]
        "mysql" => Some(Backend::Mysql),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn opens_in_memory_sqlite() {
        let pool = connect("sqlite::memory:").await.unwrap();
        assert_eq!(backend_of(&pool).await.unwrap(), Backend::Sqlite);
    }

    #[tokio::test]
    async fn rejects_unknown_scheme() {
        let err = connect("oracle://x").await.unwrap_err();
        assert!(matches!(err, StoreError::UnsupportedDsn(_)));
    }
}
