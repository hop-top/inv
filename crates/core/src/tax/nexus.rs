//! US economic-nexus configuration.
//!
//! One row per US state. The seller (US-DE) must opt the state in via
//! `enabled = true` AND have crossed either the revenue or the
//! transaction-count threshold for that state. Otherwise the tax engine
//! returns 0 % and flags `nexus_review`.
//!
//! Loaded from the same TOML file as the rate table, under `[nexus.<STATE>]`
//! tables (e.g. `[nexus.CA]`, `[nexus.NY]`).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Threshold + enabled flag for a single US state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NexusThreshold {
    /// Operator switch — false means the engine never applies this state's
    /// rate even if the threshold is crossed.
    #[serde(default)]
    pub enabled: bool,
    /// Revenue threshold (in seller currency, typically USD). `None` =
    /// revenue does not gate nexus for this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revenue: Option<Decimal>,
    /// Transaction-count threshold. `None` = txn count does not gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub txn_count: Option<u32>,
}

impl NexusThreshold {
    /// True if the seller-supplied figures cross *either* configured
    /// threshold for this state.
    ///
    /// The semantics match design §6.1 step 2 (US case): `enabled AND
    /// (seller_revenue >= revenue OR seller_txn_count >= txn_count)`.
    /// If neither threshold is configured (both `None`), the state is
    /// considered to have nexus the moment `enabled = true`. If the
    /// state is disabled, returns false regardless.
    pub fn has_nexus(&self, seller_revenue: Decimal, seller_txn_count: u32) -> bool {
        if !self.enabled {
            return false;
        }
        // If no thresholds are configured at all, "enabled" alone means nexus.
        if self.revenue.is_none() && self.txn_count.is_none() {
            return true;
        }
        let revenue_hit = self.revenue.is_some_and(|r| seller_revenue >= r);
        let txn_hit = self.txn_count.is_some_and(|c| seller_txn_count >= c);
        revenue_hit || txn_hit
    }
}

/// Per-state nexus configuration, keyed by uppercase state subdivision code
/// (e.g. `"CA"`, `"NY"`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NexusConfig {
    /// State -> threshold map.
    pub states: HashMap<String, NexusThreshold>,
}

impl NexusConfig {
    /// Empty config (no nexus anywhere).
    pub fn empty() -> Self {
        Self {
            states: HashMap::new(),
        }
    }

    /// Look up a state by its ISO 3166-2 subdivision suffix (case-insensitive).
    pub fn get(&self, state: &str) -> Option<&NexusThreshold> {
        self.states
            .get(&state.to_ascii_uppercase())
            .or_else(|| self.states.get(state))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::FromStr;

    #[test]
    fn disabled_state_never_has_nexus() {
        let t = NexusThreshold {
            enabled: false,
            revenue: Some(Decimal::from_str("100").unwrap()),
            txn_count: Some(1),
        };
        assert!(!t.has_nexus(Decimal::from_str("999999").unwrap(), 9999));
    }

    #[test]
    fn enabled_no_thresholds_always_nexus() {
        let t = NexusThreshold {
            enabled: true,
            revenue: None,
            txn_count: None,
        };
        assert!(t.has_nexus(Decimal::ZERO, 0));
    }

    #[test]
    fn either_threshold_triggers_nexus() {
        let t = NexusThreshold {
            enabled: true,
            revenue: Some(Decimal::from_str("500000").unwrap()),
            txn_count: Some(200),
        };
        // Revenue under, txn under -> no nexus.
        assert!(!t.has_nexus(Decimal::from_str("499999").unwrap(), 199));
        // Revenue hit -> nexus.
        assert!(t.has_nexus(Decimal::from_str("500000").unwrap(), 0));
        // Txn hit -> nexus.
        assert!(t.has_nexus(Decimal::ZERO, 200));
    }

    #[test]
    fn config_lookup_is_case_insensitive() {
        let mut states = HashMap::new();
        states.insert(
            "CA".to_string(),
            NexusThreshold {
                enabled: true,
                ..Default::default()
            },
        );
        let cfg = NexusConfig { states };
        assert!(cfg.get("CA").is_some());
        assert!(cfg.get("ca").is_some());
        assert!(cfg.get("ny").is_none());
    }
}
