//! A single row from the tax table.
//!
//! Rows are deserialized from TOML per design §6.2 — see
//! `tax-tables/default.toml` for the canonical shape.

use crate::domain::invoice::TaxCategory;
use crate::domain::jurisdiction::Jurisdiction;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Buyer-side filter on a tax row.
///
/// In TOML this lives under an `applies_to_buyer = { country = "CA",
/// region = "QC" }` inline table. Both fields are optional: a row with
/// only `country` set applies to any buyer in that country; a row with
/// neither key applies to any buyer (rare — usually you want at least
/// `country`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AppliesToBuyer {
    /// ISO 3166-1 alpha-2 country code (e.g. `"CA"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// ISO 3166-2 subdivision suffix without the country prefix (e.g.
    /// `"QC"`, `"CA"`, `"NY"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

/// A single configured tax rate.
///
/// One row per (jurisdiction, buyer-scope, category, effective-window)
/// tuple. Multiple rows may match a given line (e.g. QC seller selling
/// to a QC buyer matches both `ca-qc-gst` and `ca-qc-qst`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TaxRate {
    /// Stable identifier (e.g. `"ca-qc-gst"`). Recorded on each
    /// `invoice_lines.tax_rate_ids` entry for audit.
    pub id: String,
    /// Seller jurisdiction the row belongs to.
    pub jurisdiction: Jurisdiction,
    /// Buyer-side filter.
    #[serde(default, rename = "applies_to_buyer")]
    pub applies_to_buyer: AppliesToBuyer,
    /// Human-readable name (e.g. `"GST"`, `"QST"`, `"TVA"`).
    pub name: String,
    /// Tax category this row covers.
    #[serde(default)]
    pub category: TaxCategory,
    /// Rate as a fraction (e.g. `0.05` for 5 %).
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub rate: Decimal,
    /// First date the row is effective (inclusive).
    pub effective_from: NaiveDate,
    /// Last date the row is effective (inclusive). `None` = open-ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_to: Option<NaiveDate>,
}

impl TaxRate {
    /// Convenience: country filter, if any.
    pub fn applies_country(&self) -> Option<&str> {
        self.applies_to_buyer.country.as_deref()
    }

    /// Convenience: region filter, if any.
    pub fn applies_region(&self) -> Option<&str> {
        self.applies_to_buyer.region.as_deref()
    }

    /// True if `at` falls inside `[effective_from, effective_to]`.
    pub fn is_effective_on(&self, at: NaiveDate) -> bool {
        if at < self.effective_from {
            return false;
        }
        !matches!(self.effective_to, Some(end) if at > end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::FromStr;

    #[test]
    fn effective_window_inclusive() {
        let r = TaxRate {
            id: "x".into(),
            jurisdiction: Jurisdiction::QuebecCa,
            applies_to_buyer: AppliesToBuyer::default(),
            name: "X".into(),
            category: TaxCategory::Standard,
            rate: Decimal::from_str("0.05").unwrap(),
            effective_from: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            effective_to: Some(NaiveDate::from_ymd_opt(2024, 12, 31).unwrap()),
        };
        assert!(r.is_effective_on(NaiveDate::from_ymd_opt(2024, 1, 1).unwrap()));
        assert!(r.is_effective_on(NaiveDate::from_ymd_opt(2024, 6, 15).unwrap()));
        assert!(r.is_effective_on(NaiveDate::from_ymd_opt(2024, 12, 31).unwrap()));
        assert!(!r.is_effective_on(NaiveDate::from_ymd_opt(2023, 12, 31).unwrap()));
        assert!(!r.is_effective_on(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap()));
    }

    #[test]
    fn open_ended_window() {
        let r = TaxRate {
            id: "x".into(),
            jurisdiction: Jurisdiction::QuebecCa,
            applies_to_buyer: AppliesToBuyer::default(),
            name: "X".into(),
            category: TaxCategory::Standard,
            rate: Decimal::from_str("0.05").unwrap(),
            effective_from: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
            effective_to: None,
        };
        assert!(r.is_effective_on(NaiveDate::from_ymd_opt(2099, 1, 1).unwrap()));
    }
}
