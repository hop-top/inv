//! Inline TOML configuration loader.
//!
//! `inv-cli` reads a single `inv.toml` file in the kit-conventional
//! location (`$XDG_CONFIG_HOME/inv/config.toml`, falling back to
//! `$HOME/.config/inv/config.toml` on platforms without `XDG_CONFIG_HOME`)
//! or wherever `--config <path>` points. The on-disk format is a subset
//! of design §11 — only the fields the CLI actually consumes at v1 are
//! parsed; the rest are accepted and ignored so operators can keep one
//! file across adapters.
//!
//! When no file is found, the CLI falls back to an in-memory sqlite DB
//! and the bundled `tax-tables/default.toml`. A real kit config primitive
//! lands with T-0061; until then this loader is the seam.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// Errors raised while loading the config file.
#[derive(Debug)]
pub enum ConfigError {
    /// I/O failure reading the TOML file.
    Io {
        /// Path that failed.
        path: String,
        /// Underlying error.
        source: std::io::Error,
    },
    /// File did not parse.
    Parse {
        /// Path that failed.
        path: String,
        /// Underlying error.
        source: toml::de::Error,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io { path, source } => {
                write!(f, "reading config file {path}: {source}")
            }
            ConfigError::Parse { path, source } => {
                write!(f, "parsing config TOML {path}: {source}")
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io { source, .. } => Some(source),
            ConfigError::Parse { source, .. } => Some(source),
        }
    }
}

/// Top-level config shape.
#[derive(Debug, Default, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Config {
    /// Storage backend wiring.
    #[serde(default)]
    pub storage: StorageConfig,
    /// Tax engine config (table path).
    #[serde(default)]
    pub tax: TaxConfig,
    /// Bus config (advisory at v1; kept so the file format matches
    /// design §11 verbatim and operators can hand-edit a single file).
    #[serde(default)]
    pub bus: BusConfig,
}

/// `[storage]` table.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct StorageConfig {
    /// Backend identifier (`sqlite` | `postgres` | `tidb`). v1 only
    /// instantiates sqlite from the CLI; the others compile-check but
    /// are not invoked from the binary until kit primitives land.
    #[serde(default = "default_backend")]
    pub backend: String,
    /// sqlx-style DSN.
    #[serde(default = "default_dsn")]
    pub dsn: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            dsn: default_dsn(),
        }
    }
}

fn default_backend() -> String {
    "sqlite".to_string()
}

fn default_dsn() -> String {
    "sqlite::memory:".to_string()
}

/// `[tax]` table.
#[derive(Debug, Clone, Deserialize)]
pub struct TaxConfig {
    /// Path to the tax table TOML file (relative paths are resolved
    /// against the config file's parent dir; absolute paths used verbatim).
    #[serde(default = "default_tax_tables")]
    pub tax_tables: String,
}

impl Default for TaxConfig {
    fn default() -> Self {
        Self {
            tax_tables: default_tax_tables(),
        }
    }
}

fn default_tax_tables() -> String {
    "tax-tables/default.toml".to_string()
}

/// `[bus]` table — captured but unused at v1.
#[derive(Debug, Default, Clone, Deserialize)]
#[allow(dead_code)]
pub struct BusConfig {
    /// Inbound topic subscriptions.
    #[serde(default)]
    pub topics_in: Vec<String>,
}

impl Config {
    /// Resolve config from an explicit path or the kit-conventional
    /// default. Returns `Ok(None)` when no file is present.
    pub fn load(explicit: Option<&Path>) -> Result<Option<Self>, ConfigError> {
        let path = match explicit {
            Some(p) => Some(p.to_path_buf()),
            None => default_path(),
        };
        let Some(path) = path else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let cfg: Config = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.display().to_string(),
            source,
        })?;
        Ok(Some(cfg))
    }
}

/// `$XDG_CONFIG_HOME/inv/config.toml`, falling back to
/// `$HOME/.config/inv/config.toml`.
fn default_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("inv").join("config.toml"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return Some(
                PathBuf::from(home)
                    .join(".config")
                    .join("inv")
                    .join("config.toml"),
            );
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing.toml");
        let cfg = Config::load(Some(&missing)).unwrap();
        assert!(cfg.is_none());
    }

    #[test]
    fn parses_minimal_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("inv.toml");
        std::fs::write(
            &path,
            r#"
[storage]
backend = "sqlite"
dsn = "sqlite:./inv.sqlite"

[tax]
tax_tables = "tax-tables/default.toml"
"#,
        )
        .unwrap();
        let cfg = Config::load(Some(&path)).unwrap().unwrap();
        assert_eq!(cfg.storage.backend, "sqlite");
        assert_eq!(cfg.storage.dsn, "sqlite:./inv.sqlite");
        assert_eq!(cfg.tax.tax_tables, "tax-tables/default.toml");
    }

    #[test]
    fn parses_with_defaults_only() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("inv.toml");
        std::fs::write(&path, "").unwrap();
        let cfg = Config::load(Some(&path)).unwrap().unwrap();
        assert_eq!(cfg.storage.backend, "sqlite");
        assert_eq!(cfg.storage.dsn, "sqlite::memory:");
        assert_eq!(cfg.tax.tax_tables, "tax-tables/default.toml");
    }
}
