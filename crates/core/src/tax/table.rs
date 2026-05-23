//! Tax-table loader + indexed lookups.
//!
//! The TOML format is described in design §6.2 — see
//! `tax-tables/default.toml` for the canonical example.

use super::nexus::{NexusConfig, NexusThreshold};
use super::rate::TaxRate;
use crate::domain::invoice::TaxCategory;
use crate::domain::jurisdiction::Jurisdiction;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use thiserror::Error;

/// Wire-format of a `tax-tables/*.toml` file.
///
/// Two top-level keys:
/// - `rate` — array of [`TaxRate`] (TOML `[[rate]]` blocks).
/// - `nexus` — table keyed by US state code, value is a [`NexusThreshold`].
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TaxFile {
    /// Array of tax rate rows.
    #[serde(default, rename = "rate")]
    pub rates: Vec<TaxRate>,
    /// Per-state US nexus map.
    #[serde(default)]
    pub nexus: HashMap<String, NexusThreshold>,
}

/// Errors raised while loading or querying the tax table.
#[derive(Debug, Error)]
pub enum TaxTableError {
    /// I/O failure reading the TOML file.
    #[error("reading tax-table file {path:?}: {source}")]
    Io {
        /// Path that failed.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The file did not parse as a valid tax-table TOML.
    #[error("parsing tax-table TOML: {0}")]
    Parse(#[from] toml::de::Error),
}

/// Loaded and indexed tax table.
///
/// `rates` is the raw row list (order matches the TOML file).
/// Indexed lookups walk the list once; v1 expects O(dozens) rows so this
/// is cheap. If the table ever grows beyond a few hundred rows we can
/// build a `HashMap<(Jurisdiction, country, region, category), Vec<usize>>`
/// without changing the public surface.
#[derive(Debug, Default, Clone)]
pub struct TaxTable {
    /// Raw rows.
    pub rates: Vec<TaxRate>,
}

impl TaxTable {
    /// Build a table from an already-parsed [`TaxFile`].
    pub fn from_file(file: TaxFile) -> (Self, NexusConfig) {
        let table = TaxTable { rates: file.rates };
        let nexus = NexusConfig { states: file.nexus };
        (table, nexus)
    }

    /// Parse a TOML string into a table + nexus config.
    pub fn load_from_str(toml_text: &str) -> Result<(Self, NexusConfig), TaxTableError> {
        let file: TaxFile = toml::from_str(toml_text)?;
        Ok(Self::from_file(file))
    }

    /// Load from a file path.
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<(Self, NexusConfig), TaxTableError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| TaxTableError::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::load_from_str(&text)
    }

    /// Number of rows.
    pub fn len(&self) -> usize {
        self.rates.len()
    }

    /// True if no rows are loaded.
    pub fn is_empty(&self) -> bool {
        self.rates.is_empty()
    }

    /// All rows matching the given seller jurisdiction, buyer country,
    /// optional buyer region, category, and effective date.
    ///
    /// A row matches when:
    /// - `seller_jur` equals the row's jurisdiction
    /// - the row's country filter is `None` OR equals `buyer_country`
    /// - the row's region filter is `None` OR equals `buyer_region`
    ///   (a row with a region filter is skipped when the buyer's region
    ///   is unknown)
    /// - the row's category equals `category`
    /// - the row is effective on `at`
    pub fn lookup<'a>(
        &'a self,
        seller: Jurisdiction,
        buyer_country: &str,
        buyer_region: Option<&str>,
        category: TaxCategory,
        at: NaiveDate,
    ) -> Vec<&'a TaxRate> {
        self.rates
            .iter()
            .filter(|r| r.jurisdiction == seller)
            .filter(|r| match r.applies_country() {
                Some(c) => c.eq_ignore_ascii_case(buyer_country),
                None => true,
            })
            .filter(|r| match (r.applies_region(), buyer_region) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(want), Some(have)) => want.eq_ignore_ascii_case(have),
            })
            .filter(|r| r.category == category)
            .filter(|r| r.is_effective_on(at))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
[[rate]]
id = "ca-qc-gst"
jurisdiction = "CA-QC"
applies_to_buyer = { country = "CA" }
name = "GST"
category = "standard"
rate = "0.05"
effective_from = "2008-01-01"

[[rate]]
id = "ca-qc-qst"
jurisdiction = "CA-QC"
applies_to_buyer = { country = "CA", region = "QC" }
name = "QST"
category = "standard"
rate = "0.09975"
effective_from = "2013-01-01"

[nexus.CA]
enabled = true
revenue = "500000"
"#;

    #[test]
    fn parses_fixture() {
        let (table, nexus) = TaxTable::load_from_str(FIXTURE).unwrap();
        assert_eq!(table.len(), 2);
        assert!(nexus.get("CA").unwrap().enabled);
    }

    #[test]
    fn qc_to_qc_returns_gst_and_qst() {
        let (table, _) = TaxTable::load_from_str(FIXTURE).unwrap();
        let hits = table.lookup(
            Jurisdiction::QuebecCa,
            "CA",
            Some("QC"),
            TaxCategory::Standard,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        );
        assert_eq!(hits.len(), 2);
        let ids: Vec<&str> = hits.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"ca-qc-gst"));
        assert!(ids.contains(&"ca-qc-qst"));
    }

    #[test]
    fn qc_to_on_returns_only_gst() {
        let (table, _) = TaxTable::load_from_str(FIXTURE).unwrap();
        let hits = table.lookup(
            Jurisdiction::QuebecCa,
            "CA",
            Some("ON"),
            TaxCategory::Standard,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        );
        // No QC-seller-to-ON-buyer-specific row in fixture; only the
        // country-wide GST row matches.
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "ca-qc-gst");
    }

    #[test]
    fn region_required_row_skipped_when_buyer_region_unknown() {
        let (table, _) = TaxTable::load_from_str(FIXTURE).unwrap();
        // Buyer in CA but no region -> QST (region=QC) must NOT match.
        let hits = table.lookup(
            Jurisdiction::QuebecCa,
            "CA",
            None,
            TaxCategory::Standard,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "ca-qc-gst");
    }

    #[test]
    fn category_filter_works() {
        let (table, _) = TaxTable::load_from_str(FIXTURE).unwrap();
        let hits = table.lookup(
            Jurisdiction::QuebecCa,
            "CA",
            Some("QC"),
            TaxCategory::Reduced,
            NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
        );
        assert!(hits.is_empty());
    }

    #[test]
    fn default_table_file_parses() {
        // The shipped tax-tables/default.toml must parse cleanly. Use a
        // CARGO_MANIFEST_DIR-relative path so the test works under any
        // workspace cwd.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = std::path::Path::new(manifest_dir)
            .join("../..")
            .join("tax-tables/default.toml");
        let (table, nexus) = TaxTable::load_from_file(&path)
            .unwrap_or_else(|e| panic!("loading {}: {e}", path.display()));
        assert!(
            table.len() >= 5,
            "expected several rate rows, got {}",
            table.len()
        );
        // Every shipped US nexus state must default to disabled.
        for (state, t) in &nexus.states {
            assert!(!t.enabled, "shipped default has nexus.{state} enabled");
        }
    }

    #[test]
    fn effective_window_filters() {
        let (table, _) = TaxTable::load_from_str(FIXTURE).unwrap();
        // GST started 2008-01-01; 2007 lookups should miss.
        let hits = table.lookup(
            Jurisdiction::QuebecCa,
            "CA",
            None,
            TaxCategory::Standard,
            NaiveDate::from_ymd_opt(2007, 12, 31).unwrap(),
        );
        assert!(hits.is_empty());
    }
}
