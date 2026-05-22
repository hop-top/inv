//! Currency newtype + money type aliases.
//!
//! v1 stores every amount as [`rust_decimal::Decimal`]. Per the design
//! spec, money columns serialize to sqlite as `TEXT` (Decimal's canonical
//! string), so [`Decimal`]'s built-in `Serialize`/`Deserialize` via the
//! `serde-with-str` feature is what wire-form payloads see.
//!
//! Currency is a 3-letter ISO 4217 code wrapped in a newtype. v1 supports
//! USD, CAD, DZD across our three markets; other codes deserialize fine
//! but won't have minor-unit metadata until added to [`Currency::scale`].
//!
//! Rounding follows banker's (round-half-to-even) per the design spec.

use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// ISO 4217 currency code (always uppercase, 3 letters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Currency([u8; 3]);

// Wire form is the 3-letter string (see `serde(into/try_from)` above); the
// schema mirrors that — not the raw `[u8; 3]` byte array. We cannot derive
// `JsonSchema` here because serde's `transparent`/`into`/`try_from` and
// schemars's matching attrs don't co-exist on the same struct in v1.x.
#[cfg(feature = "schema")]
impl schemars::JsonSchema for Currency {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Currency".into()
    }
    fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <String as schemars::JsonSchema>::json_schema(g)
    }
}

/// Errors from constructing a [`Currency`].
#[derive(Debug, Error)]
pub enum CurrencyError {
    /// Code wasn't exactly 3 ASCII letters.
    #[error("invalid ISO 4217 code: `{0}` (must be 3 ASCII letters)")]
    Invalid(String),
}

impl Currency {
    /// US dollar.
    pub const USD: Currency = Currency(*b"USD");
    /// Canadian dollar.
    pub const CAD: Currency = Currency(*b"CAD");
    /// Algerian dinar.
    pub const DZD: Currency = Currency(*b"DZD");

    /// Construct from a 3-letter ISO 4217 code (case-insensitive).
    pub fn new(code: &str) -> Result<Self, CurrencyError> {
        if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err(CurrencyError::Invalid(code.to_string()));
        }
        let mut bytes = [0u8; 3];
        for (i, b) in code.bytes().enumerate() {
            bytes[i] = b.to_ascii_uppercase();
        }
        Ok(Currency(bytes))
    }

    /// The ISO 4217 code as a `&str`.
    pub fn as_str(&self) -> &str {
        // Safe: every byte is ASCII uppercase (enforced at construction).
        std::str::from_utf8(&self.0).expect("ascii uppercase")
    }

    /// Number of decimal places for this currency's minor unit.
    ///
    /// v1 hard-codes USD/CAD = 2, DZD = 0. Unknown currencies default to 2
    /// (most-common scale); promote a currency by adding a match arm.
    pub fn scale(&self) -> u32 {
        match self.as_str() {
            "DZD" => 0,
            "USD" | "CAD" => 2,
            _ => 2,
        }
    }

    /// Round a [`Decimal`] to this currency's scale using banker's rounding.
    pub fn round(&self, amount: Decimal) -> Decimal {
        amount.round_dp_with_strategy(self.scale(), RoundingStrategy::MidpointNearestEven)
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<Currency> for String {
    fn from(c: Currency) -> String {
        c.as_str().to_string()
    }
}

impl TryFrom<String> for Currency {
    type Error = CurrencyError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Currency::new(&s)
    }
}

impl std::str::FromStr for Currency {
    type Err = CurrencyError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Currency::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::FromStr;

    #[test]
    fn known_currencies_round_trip() {
        for c in [Currency::USD, Currency::CAD, Currency::DZD] {
            let s = c.to_string();
            let back: Currency = s.parse().unwrap();
            assert_eq!(c, back);
        }
    }

    #[test]
    fn lowercase_is_normalized() {
        let c: Currency = "cad".parse().unwrap();
        assert_eq!(c, Currency::CAD);
        assert_eq!(c.to_string(), "CAD");
    }

    #[test]
    fn invalid_codes_rejected() {
        assert!("US".parse::<Currency>().is_err());
        assert!("USDC".parse::<Currency>().is_err());
        assert!("US1".parse::<Currency>().is_err());
        assert!("".parse::<Currency>().is_err());
    }

    #[test]
    fn scale_known_currencies() {
        assert_eq!(Currency::USD.scale(), 2);
        assert_eq!(Currency::CAD.scale(), 2);
        assert_eq!(Currency::DZD.scale(), 0);
    }

    #[test]
    fn bankers_rounding() {
        // 2.5 → 2.0 (round half to even)
        // 3.5 → 4.0
        // 2.345 at scale 2 → 2.34 (5 rounds to even)
        // 2.355 at scale 2 → 2.36
        let usd = Currency::USD;
        assert_eq!(usd.round(Decimal::from_str("2.5").unwrap()), Decimal::from_str("2.5").unwrap()); // scale=2 keeps it
        assert_eq!(usd.round(Decimal::from_str("2.345").unwrap()), Decimal::from_str("2.34").unwrap());
        assert_eq!(usd.round(Decimal::from_str("2.355").unwrap()), Decimal::from_str("2.36").unwrap());

        let dzd = Currency::DZD;
        assert_eq!(dzd.round(Decimal::from_str("2.5").unwrap()), Decimal::from_str("2").unwrap()); // round to even
        assert_eq!(dzd.round(Decimal::from_str("3.5").unwrap()), Decimal::from_str("4").unwrap());
    }

    #[test]
    fn serde_via_string() {
        let c = Currency::CAD;
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, "\"CAD\"");
        let back: Currency = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn decimal_serde_is_string_form() {
        // Confirm rust_decimal with `serde-with-str` serializes as JSON strings,
        // not floats — this is load-bearing for sqlite TEXT round-trips.
        let d = Decimal::from_str("123.456789").unwrap();
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, "\"123.456789\"");
        let back: Decimal = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }
}
