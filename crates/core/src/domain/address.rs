//! Postal addresses used for tax destination + invoice rendering.
//!
//! Stored as JSON in the `customers.address_json` column, per design §5.
//!
//! The address shape is deliberately minimal: `country` is required and
//! is ISO 3166-1 alpha-2 (used by the tax engine to determine buyer
//! scope). `region` is optional but recommended (ISO 3166-2 subdivision,
//! e.g. `QC`, `CA`, `NY`) — without it, US economic-nexus inference
//! cannot run and the tax engine flags the invoice for operator review.

use serde::{Deserialize, Serialize};

/// A postal address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    /// ISO 3166-1 alpha-2 country code (e.g. `"CA"`, `"US"`, `"DZ"`).
    pub country: String,
    /// ISO 3166-2 subdivision code without the country prefix (e.g.
    /// `"QC"` for Quebec, `"DE"` for Delaware, `"16"` for Algiers).
    ///
    /// Optional but recommended: required for sub-national tax inference
    /// (Canadian provincial sales tax, US economic-nexus).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// City name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    /// Postal code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postal: Option<String>,
    /// First address line (street + number).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line1: Option<String>,
    /// Second address line (apartment, suite, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line2: Option<String>,
}

impl Address {
    /// Construct an address with just the country (the tax-engine minimum).
    pub fn country_only(country: impl Into<String>) -> Self {
        Address {
            country: country.into(),
            region: None,
            city: None,
            postal: None,
            line1: None,
            line2: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_address_round_trips() {
        let a = Address::country_only("CA");
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, r#"{"country":"CA"}"#);
        let back: Address = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn full_address_round_trips() {
        let a = Address {
            country: "US".into(),
            region: Some("DE".into()),
            city: Some("Wilmington".into()),
            postal: Some("19801".into()),
            line1: Some("100 Market St".into()),
            line2: Some("Suite 500".into()),
        };
        let json = serde_json::to_string(&a).unwrap();
        let back: Address = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn missing_optionals_are_omitted_from_json() {
        let a = Address {
            country: "DZ".into(),
            region: Some("16".into()),
            city: None,
            postal: None,
            line1: None,
            line2: None,
        };
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(json, r#"{"country":"DZ","region":"16"}"#);
    }
}
