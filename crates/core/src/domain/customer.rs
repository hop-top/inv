//! Customer record.
//!
//! `inv` does not manage customers — they're external references. The
//! `display_name`, `email`, and `address` are stored so invoices can be
//! addressed and rendered; tax destination is inferred from `address`.

use super::address::Address;
use super::ids::CustomerId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A customer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Customer {
    /// Stable identifier.
    pub id: CustomerId,
    /// Display name (rendered on invoices).
    pub display_name: String,
    /// Email (optional; used by delivery channels that send via email).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Postal address. Country is required; region is recommended (see
    /// [`Address`]).
    pub address: Address,
    /// Free-form metadata bag (caller-defined keys).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// First seen by `inv` (DB insert time).
    pub created_at: DateTime<Utc>,
    /// Last modified by `inv`.
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn customer_round_trips() {
        let c = Customer {
            id: CustomerId::new(),
            display_name: "Acme Corp".into(),
            email: Some("billing@acme.example".into()),
            address: Address {
                country: "CA".into(),
                region: Some("QC".into()),
                city: Some("Montréal".into()),
                postal: None,
                line1: None,
                line2: None,
            },
            metadata: BTreeMap::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: Customer = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }
}
