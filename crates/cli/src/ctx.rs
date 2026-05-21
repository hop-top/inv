//! Build a [`CoreCtx`] from a config file (or its in-memory fallback).
//!
//! The CLI is the only adapter that needs to bootstrap everything from
//! scratch — server adapters (api/ws/mcp at T-0017..T-0019) will reuse
//! an already-constructed `CoreCtx` injected by the server compositor
//! (T-0020). Until then this module is the one place that knows how to
//! glue config → store → tax table → ctx.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};

use inv_commands::{CoreCtx, SystemClock};
use inv_core::tax::TaxTable;
use inv_store::{connect, run_migrations};

use crate::config::Config;

/// Resolve the tax-tables path: explicit config value > `$INV_TAX_TABLES`
/// env var > a workspace-relative fallback (`tax-tables/default.toml`).
fn tax_table_path(cfg: &Config, config_dir: Option<&PathBuf>) -> PathBuf {
    if let Ok(env_path) = std::env::var("INV_TAX_TABLES") {
        if !env_path.is_empty() {
            return PathBuf::from(env_path);
        }
    }
    let raw = PathBuf::from(&cfg.tax.tax_tables);
    if raw.is_absolute() {
        return raw;
    }
    if let Some(dir) = config_dir {
        return dir.join(&raw);
    }
    raw
}

/// Build a `CoreCtx` against a (possibly missing) on-disk config.
///
/// When no config is supplied the CLI falls back to an in-memory sqlite
/// pool + the bundled default tax table. We run migrations on every
/// build so the in-memory pool comes up usable for `list`-style commands
/// that should print `[]` against an empty DB.
pub async fn build(cfg: Option<Config>, config_dir: Option<PathBuf>) -> Result<CoreCtx> {
    let cfg = cfg.unwrap_or_default();

    // 1. DB pool + migrations.
    let pool = connect(&cfg.storage.dsn)
        .await
        .with_context(|| format!("connecting to {}", cfg.storage.dsn))?;
    run_migrations(&pool)
        .await
        .with_context(|| format!("running migrations on {}", cfg.storage.dsn))?;

    // 2. Tax table.
    let tax_path = tax_table_path(&cfg, config_dir.as_ref());
    let (table, nexus) = TaxTable::load_from_file(&tax_path)
        .map_err(|e| anyhow!("loading tax table {}: {e}", tax_path.display()))?;

    // 3. Build context with the default SystemClock.
    let mut ctx = CoreCtx::new(pool, table, nexus);
    ctx.clock = Arc::new(SystemClock);
    Ok(ctx)
}
