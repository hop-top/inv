//! Cross-crate test helpers. Gated behind the `test-fixtures` feature so
//! production builds don't pull this in.
//!
//! The single helper [`connect_for_tests`] lets the same integration
//! suite run against sqlite (default — hermetic, fast) or postgres (CI
//! `test-postgres` job; opt in by setting `DATABASE_URL=postgres://…`).
//!
//! Consumer crates that need fixtures (api, bus, commands, e2e, mcp, ws,
//! and store itself) add to their `[dev-dependencies]`:
//!
//! ```toml
//! hop-top-inv-store = { workspace = true, features = ["test-fixtures"] }
//! ```
//!
//! …and replace hand-rolled `connect("sqlite::memory:") + run_migrations`
//! pairs with a single `connect_for_tests().await`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::any::{install_default_drivers, AnyPoolOptions};
use sqlx::Executor;

use crate::error::Result;
use crate::migrate::run_migrations;
use crate::pool::{connect, Pool};

/// Per-process monotonic counter used by the postgres path to mint a
/// unique schema per test invocation. Combined with a process-time
/// nonce so concurrent cargo-test binaries don't collide.
static SCHEMA_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Connect to `DATABASE_URL` if set, else `sqlite::memory:`. Runs
/// migrations against the chosen backend.
///
/// Per-test isolation:
/// - **sqlite**: each call opens a fresh `:memory:` pool — naturally
///   isolated, nothing to clean up.
/// - **postgres**: each call creates a unique schema
///   (`inv_test_<unix>_<pid>_<n>`), pins the pool's `search_path` to
///   it, then runs migrations into that schema. Strategy (c) from the
///   T-0050 plan: tolerates parallel `#[tokio::test]` execution within
///   and across test binaries — a hard requirement, since cargo runs
///   tests in parallel by default and the CI postgres job doesn't pass
///   `--test-threads=1`.
///
/// Returns a ready-to-use pool with the full migration set applied.
pub async fn connect_for_tests() -> Result<Pool> {
    let dsn = std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite::memory:".to_string());

    if is_postgres_dsn(&dsn) {
        connect_postgres_with_unique_schema(&dsn).await
    } else {
        let pool = connect(&dsn).await?;
        run_migrations(&pool).await?;
        Ok(pool)
    }
}

fn is_postgres_dsn(dsn: &str) -> bool {
    let lower = dsn.to_ascii_lowercase();
    lower.starts_with("postgres://") || lower.starts_with("postgresql://")
}

/// Open a postgres pool that mints + pins a fresh `inv_test_<nonce>`
/// schema, then runs migrations into it. The schema is created via a
/// throwaway admin connection; `search_path` is set on every checked-out
/// connection via `after_connect`, so the migrator and every repository
/// query see the test-private schema.
async fn connect_postgres_with_unique_schema(dsn: &str) -> Result<Pool> {
    install_default_drivers();

    let schema = mint_schema_name();

    // Step 1: open an admin connection to create the schema. We use a
    // 1-conn pool, run the DDL, then drop it.
    let admin = AnyPoolOptions::new()
        .max_connections(1)
        .connect(dsn)
        .await?;
    let create = format!("CREATE SCHEMA IF NOT EXISTS {schema}");
    admin.execute(create.as_str()).await?;
    drop(admin);

    // Step 2: open the real pool. `search_path` is pinned in
    // `after_connect` so every connection sqlx hands out — including
    // those used by the migrator — writes/reads from the test schema.
    //
    // `set_path` is held via `Arc<str>` so the `Fn`-bound callback can
    // cheaply clone it for each invocation.
    let set_path: Arc<str> = Arc::from(format!("SET search_path TO {schema}"));
    let pool = AnyPoolOptions::new()
        .max_connections(5)
        .after_connect(move |conn, _meta| {
            let stmt = set_path.clone();
            Box::pin(async move {
                conn.execute(stmt.as_ref()).await?;
                Ok(())
            })
        })
        .connect(dsn)
        .await?;

    run_migrations(&pool).await?;
    Ok(pool)
}

/// Build a process-unique schema identifier safe for postgres:
/// `inv_test_<unix_seconds>_<pid>_<counter>`. Lowercase + underscores
/// only — no quoting required.
fn mint_schema_name() -> String {
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let pid = std::process::id();
    let n = SCHEMA_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("inv_test_{unix}_{pid}_{n}")
}
