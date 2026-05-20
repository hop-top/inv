//! Run migrations against an open [`Pool`], picking the right migration
//! directory for the pool's backend.
//!
//! `sqlx::migrate!` is a build-time macro and embeds files into the
//! binary. We embed both `migrations/sqlite/` and `migrations/postgres/`
//! and dispatch on the backend at runtime. (`tidb` shares the postgres
//! shape with type translation that v1 hasn't authored yet — feature is
//! reserved but currently re-uses the postgres migration set; that's
//! acceptable for the compile-only verification step requested at v1
//! and will be replaced with `migrations/mysql/` when the time comes.)

use sqlx::migrate::Migrator;

use crate::error::Result;
use crate::pool::{backend_of, Backend, Pool};

#[cfg(feature = "sqlite")]
static SQLITE_MIGRATOR: Migrator = sqlx::migrate!("./migrations/sqlite");

#[cfg(any(feature = "postgres", feature = "tidb"))]
static POSTGRES_MIGRATOR: Migrator = sqlx::migrate!("./migrations/postgres");

/// Apply pending migrations to `pool`. Idempotent; safe to call on
/// every startup.
pub async fn run_migrations(pool: &Pool) -> Result<()> {
    match backend_of(pool).await? {
        #[cfg(feature = "sqlite")]
        Backend::Sqlite => {
            SQLITE_MIGRATOR.run(pool).await?;
        }
        #[cfg(feature = "postgres")]
        Backend::Postgres => {
            POSTGRES_MIGRATOR.run(pool).await?;
        }
        #[cfg(feature = "tidb")]
        Backend::Mysql => {
            // TODO(T-0008-followup): author a mysql/tidb-flavoured
            // migration directory (NUMERIC + TIMESTAMP + BOOLEAN tweaks).
            // Until then, attempting to migrate a tidb pool errors out
            // rather than running the postgres DDL (which would fail at
            // parse time anyway).
            return Err(crate::error::StoreError::Other(
                "tidb migrations not authored yet (see T-0008 follow-up)".into(),
            ));
        }
        // The match must remain exhaustive against `Backend`, but the
        // reachable arms are fully gated by features above. If no
        // feature was active the lib.rs `compile_error!` would have
        // already tripped.
        #[allow(unreachable_patterns)]
        _ => unreachable!("Backend variant without a feature flag"),
    }
    Ok(())
}
