//! Seller jurisdictions supported at v1.
//!
//! Three sellers across three markets:
//!
//! | Jurisdiction | ISO 3166-2 | Market |
//! |---|---|---|
//! | Quebec    | `CA-QC` | Canada |
//! | Wilmington (Delaware) | `US-DE` | United States |
//! | Algiers   | `DZ-16` | Algeria |
//!
//! The full ISO 3166-2 subdivision code lives on disk; we restrict the
//! Rust type to the three we ship rates for so the tax engine never
//! sees an unsupported jurisdiction.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// Seller jurisdiction (ISO 3166-2 subdivision).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum Jurisdiction {
    /// Quebec, Canada (`CA-QC`).
    #[serde(rename = "CA-QC")]
    QuebecCa,
    /// Delaware (Wilmington), United States (`US-DE`).
    #[serde(rename = "US-DE")]
    DelawareUs,
    /// Algiers, Algeria (`DZ-16`).
    #[serde(rename = "DZ-16")]
    AlgiersDz,
}

/// Errors from parsing a [`Jurisdiction`] from a string.
#[derive(Debug, Error)]
#[error("unsupported jurisdiction: `{0}` (v1 supports CA-QC | US-DE | DZ-16)")]
pub struct JurisdictionError(pub String);

impl Jurisdiction {
    /// ISO 3166-2 code (e.g., `"CA-QC"`).
    pub const fn as_str(&self) -> &'static str {
        match self {
            Jurisdiction::QuebecCa => "CA-QC",
            Jurisdiction::DelawareUs => "US-DE",
            Jurisdiction::AlgiersDz => "DZ-16",
        }
    }

    /// ISO 3166-1 alpha-2 country code (e.g., `"CA"`).
    pub const fn country(&self) -> &'static str {
        match self {
            Jurisdiction::QuebecCa => "CA",
            Jurisdiction::DelawareUs => "US",
            Jurisdiction::AlgiersDz => "DZ",
        }
    }
}

impl fmt::Display for Jurisdiction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Jurisdiction {
    type Err = JurisdictionError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "CA-QC" => Ok(Jurisdiction::QuebecCa),
            "US-DE" => Ok(Jurisdiction::DelawareUs),
            "DZ-16" => Ok(Jurisdiction::AlgiersDz),
            other => Err(JurisdictionError(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_all_variants() {
        for j in [
            Jurisdiction::QuebecCa,
            Jurisdiction::DelawareUs,
            Jurisdiction::AlgiersDz,
        ] {
            let s = j.to_string();
            let back: Jurisdiction = s.parse().unwrap();
            assert_eq!(j, back);
        }
    }

    #[test]
    fn country_codes() {
        assert_eq!(Jurisdiction::QuebecCa.country(), "CA");
        assert_eq!(Jurisdiction::DelawareUs.country(), "US");
        assert_eq!(Jurisdiction::AlgiersDz.country(), "DZ");
    }

    #[test]
    fn unsupported_codes_rejected() {
        assert!("US-NY".parse::<Jurisdiction>().is_err());
        assert!("CA-ON".parse::<Jurisdiction>().is_err());
        assert!("GB-LND".parse::<Jurisdiction>().is_err());
        assert!("".parse::<Jurisdiction>().is_err());
    }

    #[test]
    fn serde_uses_iso_code() {
        let j = Jurisdiction::QuebecCa;
        let json = serde_json::to_string(&j).unwrap();
        assert_eq!(json, "\"CA-QC\"");
        let back: Jurisdiction = serde_json::from_str(&json).unwrap();
        assert_eq!(j, back);
    }
}
