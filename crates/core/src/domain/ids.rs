//! Entity ID facade.
//!
//! Wraps the [`mti`] crate so the rest of `hop-top-inv-core` (and downstream
//! crates) never touch `mti` directly. When `hop_top_kit::id` lands
//! (see `hop-top/poly-kit#id-typeid`) the body of this module swaps
//! to kit's API in one place; consumers don't change.
//!
//! Each entity has its own newtype:
//!
//! ```ignore
//! use hop_top_inv_core::domain::ids::InvoiceId;
//! let id = InvoiceId::new();
//! // id.to_string() => "invoice_01j5xk..."
//! // id.as_uri()     => "inv://invoice/invoice_01j5xk..."
//! ```
//!
//! URIs follow the `hop-top/poly-uri` canonical form
//! `<scheme>://<namespace>/<id>`. Per inv's URI policy:
//!
//! - `scheme` is always `inv`
//! - `namespace` is the entity-type (matches the TypeID prefix)
//! - `id` is the full typeid string with prefix retained
//!
//! Keeping the prefix inside the id segment is deliberate: bare typeids
//! remain self-describing in logs and support contexts.

use mti::prelude::*;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// URI scheme used by every inv entity URI.
pub const URI_SCHEME: &str = "inv";

/// Errors from parsing a typeid string.
#[derive(Debug, Error)]
pub enum IdError {
    /// The string isn't a syntactically valid typeid.
    #[error("invalid typeid: {0}")]
    Invalid(String),
    /// The typeid prefix didn't match what this entity expects.
    #[error("prefix mismatch: expected `{expected}`, got `{got}`")]
    PrefixMismatch {
        /// The prefix this newtype requires.
        expected: &'static str,
        /// The prefix actually parsed from the string.
        got: String,
    },
}

/// Internal helper — produces a fresh UUIDv7-backed typeid for the given prefix.
fn new_typeid(prefix: &'static str) -> MagicTypeId {
    prefix.create_type_id::<V7>()
}

/// Internal helper — parses a typeid string and enforces the expected prefix.
fn parse_typeid(s: &str, expected_prefix: &'static str) -> Result<MagicTypeId, IdError> {
    let parsed = MagicTypeId::from_str(s).map_err(|e| IdError::Invalid(e.to_string()))?;
    let got = parsed.prefix().as_str();
    if got != expected_prefix {
        return Err(IdError::PrefixMismatch {
            expected: expected_prefix,
            got: got.to_string(),
        });
    }
    Ok(parsed)
}

/// Defines an entity ID newtype around `MagicTypeId`.
///
/// Each entity gets a `pub const PREFIX: &str` (the typeid prefix and
/// uri-poly namespace, matching by design), plus the standard
/// constructors, parsers, `Display`, serde, and `as_uri()`.
macro_rules! define_entity_id {
    (
        $(#[$meta:meta])*
        $name:ident,
        prefix = $prefix:literal $(,)?
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(MagicTypeId);

        impl $name {
            /// The typeid prefix and uri-poly namespace for this entity.
            pub const PREFIX: &'static str = $prefix;

            /// Generate a fresh ID (UUIDv7 suffix, sortable by creation time).
            pub fn new() -> Self {
                Self(new_typeid($prefix))
            }

            /// Parse a typeid string, enforcing the expected prefix.
            pub fn parse(s: &str) -> Result<Self, IdError> {
                parse_typeid(s, $prefix).map(Self)
            }

            /// Borrow the underlying `MagicTypeId` (e.g., for direct API calls).
            pub fn as_typeid(&self) -> &MagicTypeId {
                &self.0
            }

            /// Render as a canonical `hop-top/poly-uri` URI:
            /// `inv://<prefix>/<typeid>`.
            pub fn as_uri(&self) -> String {
                format!("{}://{}/{}", URI_SCHEME, $prefix, self.0)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl FromStr for $name {
            type Err = IdError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
                ser.collect_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
                let s = String::deserialize(de)?;
                Self::parse(&s).map_err(serde::de::Error::custom)
            }
        }

        #[cfg(feature = "schema")]
        impl schemars::JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }
            fn json_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
                // Wire form is the bare typeid string (see `Serialize` above).
                <String as schemars::JsonSchema>::json_schema(g)
            }
        }
    };
}

define_entity_id! {
    /// Invoice identifier (prefix `invoice`).
    InvoiceId, prefix = "invoice"
}

define_entity_id! {
    /// Customer identifier (prefix `customer`).
    CustomerId, prefix = "customer"
}

define_entity_id! {
    /// Credit-note identifier (prefix `creditnote`).
    CreditNoteId, prefix = "creditnote"
}

define_entity_id! {
    /// Recurring schedule identifier (prefix `schedule`).
    ScheduleId, prefix = "schedule"
}

define_entity_id! {
    /// Reminder identifier (prefix `reminder`).
    ReminderId, prefix = "reminder"
}

define_entity_id! {
    /// Invoice-line identifier (prefix `line`).
    LineId, prefix = "line"
}

define_entity_id! {
    /// Invoice-state-history entry identifier (prefix `history`).
    HistoryId, prefix = "history"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invoice_id_round_trips_through_string() {
        let id = InvoiceId::new();
        let s = id.to_string();
        assert!(s.starts_with("invoice_"), "got {s}");
        let parsed = InvoiceId::parse(&s).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn invoice_id_uri_form() {
        let id = InvoiceId::new();
        let uri = id.as_uri();
        let typeid = id.to_string();
        assert_eq!(uri, format!("inv://invoice/{typeid}"));
    }

    #[test]
    fn parse_rejects_wrong_prefix() {
        let cust = CustomerId::new();
        // Trying to parse a customer typeid as an InvoiceId must fail.
        let err = InvoiceId::parse(&cust.to_string()).unwrap_err();
        match err {
            IdError::PrefixMismatch { expected, got } => {
                assert_eq!(expected, "invoice");
                assert_eq!(got, "customer");
            }
            other => panic!("expected PrefixMismatch, got {other:?}"),
        }
    }

    #[test]
    fn parse_rejects_garbage() {
        let err = InvoiceId::parse("definitely_not_a_typeid").unwrap_err();
        assert!(matches!(err, IdError::Invalid(_)));
    }

    #[test]
    fn serde_round_trip() {
        let id = InvoiceId::new();
        let json = serde_json::to_string(&id).unwrap();
        // Wire form is the bare typeid string, not the URI.
        assert_eq!(json, format!("\"{id}\""));
        let back: InvoiceId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn each_entity_has_its_own_prefix() {
        let p = (
            InvoiceId::PREFIX,
            CustomerId::PREFIX,
            CreditNoteId::PREFIX,
            ScheduleId::PREFIX,
            ReminderId::PREFIX,
            LineId::PREFIX,
            HistoryId::PREFIX,
        );
        assert_eq!(p.0, "invoice");
        assert_eq!(p.1, "customer");
        assert_eq!(p.2, "creditnote");
        assert_eq!(p.3, "schedule");
        assert_eq!(p.4, "reminder");
        assert_eq!(p.5, "line");
        assert_eq!(p.6, "history");
    }

    #[test]
    fn time_sortable_under_v7() {
        // Two IDs created in sequence should sort lexicographically.
        let a = InvoiceId::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = InvoiceId::new();
        assert!(
            a.to_string() < b.to_string(),
            "v7 must be sortable: {a} vs {b}"
        );
    }
}
