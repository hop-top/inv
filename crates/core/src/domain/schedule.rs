//! Recurring schedules + their cadence grammar.
//!
//! A schedule is a recipe for materialising draft invoices. A ticker
//! (T-0013) advances `next_run` and creates a fresh draft per cycle.
//!
//! Cadence grammar (per design §8):
//!
//! | Form | Example | Meaning |
//! |---|---|---|
//! | `monthly@<dom>`     | `monthly@1`           | First of every month |
//! | `quarterly@<dom>`   | `quarterly@15`        | 15th of every 3rd month |
//! | `yearly@<MM-DD>`    | `yearly@01-01`        | January 1st every year |

use super::ids::{CustomerId, LineId, ScheduleId};
use super::invoice::TaxCategory;
use super::money::Currency;
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// Cadence at which a [`Schedule`] materialises new invoices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cadence {
    /// Once per month, on day-of-month `dom` (1..=28; 29-31 floored to month length).
    Monthly { dom: u8 },
    /// Once per quarter, on day-of-month `dom` of the first month in each quarter.
    Quarterly { dom: u8 },
    /// Once per year, on `(month, day)`.
    Yearly { month: u8, day: u8 },
}

/// Errors parsing a [`Cadence`] string.
#[derive(Debug, Error)]
pub enum CadenceError {
    /// Missing the `@` separator.
    #[error("missing `@<arg>` suffix in cadence: {0}")]
    MissingArg(String),
    /// Day-of-month out of range or not numeric.
    #[error("invalid day-of-month: {0}")]
    InvalidDom(String),
    /// Yearly `MM-DD` malformed.
    #[error("invalid yearly date `MM-DD`: {0}")]
    InvalidYearly(String),
    /// Unrecognised prefix (must be `monthly`, `quarterly`, or `yearly`).
    #[error("unrecognised cadence prefix: {0}")]
    UnknownPrefix(String),
}

impl Cadence {
    fn parse_dom(s: &str) -> Result<u8, CadenceError> {
        let dom: u8 = s.parse().map_err(|_| CadenceError::InvalidDom(s.to_string()))?;
        if !(1..=28).contains(&dom) {
            // Restricting to 1..=28 keeps every month valid without special-casing
            // February. Tools needing end-of-month should add a "last day" form later.
            return Err(CadenceError::InvalidDom(s.to_string()));
        }
        Ok(dom)
    }
}

impl fmt::Display for Cadence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Cadence::Monthly { dom } => write!(f, "monthly@{dom}"),
            Cadence::Quarterly { dom } => write!(f, "quarterly@{dom}"),
            Cadence::Yearly { month, day } => write!(f, "yearly@{month:02}-{day:02}"),
        }
    }
}

impl FromStr for Cadence {
    type Err = CadenceError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (prefix, arg) = s
            .split_once('@')
            .ok_or_else(|| CadenceError::MissingArg(s.to_string()))?;
        match prefix {
            "monthly" => Ok(Cadence::Monthly { dom: Self::parse_dom(arg)? }),
            "quarterly" => Ok(Cadence::Quarterly { dom: Self::parse_dom(arg)? }),
            "yearly" => {
                // arg is `MM-DD`
                let (mm, dd) = arg
                    .split_once('-')
                    .ok_or_else(|| CadenceError::InvalidYearly(arg.to_string()))?;
                let month: u8 = mm
                    .parse()
                    .map_err(|_| CadenceError::InvalidYearly(arg.to_string()))?;
                let day: u8 = dd
                    .parse()
                    .map_err(|_| CadenceError::InvalidYearly(arg.to_string()))?;
                if !(1..=12).contains(&month) || !(1..=28).contains(&day) {
                    return Err(CadenceError::InvalidYearly(arg.to_string()));
                }
                Ok(Cadence::Yearly { month, day })
            }
            other => Err(CadenceError::UnknownPrefix(other.to_string())),
        }
    }
}

impl Serialize for Cadence {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Cadence {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Lifecycle state of a [`Schedule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleState {
    /// Actively materialising invoices on every cycle.
    Active,
    /// Paused; the ticker skips it.
    Paused,
    /// Cancelled — terminal.
    Cancelled,
}

/// A line template for the schedule — same shape as an invoice line but
/// without identifiers or computed totals (those land on the materialised
/// invoice each cycle).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleLine {
    /// Free-form description.
    pub description: String,
    /// Quantity.
    pub quantity: Decimal,
    /// Per-unit price.
    pub unit_price: Decimal,
    /// Tax category override (default `Standard`).
    #[serde(default)]
    pub tax_category: TaxCategory,
    /// Stable line id within the schedule (so updates can target a
    /// specific template line). Not the same as the materialised
    /// invoice's `LineId`.
    pub id: LineId,
}

/// A recurring schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    /// Stable identifier.
    pub id: ScheduleId,
    /// Bill-to customer for every materialised invoice.
    pub customer_id: CustomerId,
    /// Line templates instantiated each cycle.
    pub template_lines: Vec<ScheduleLine>,
    /// Currency for every materialised invoice.
    pub currency: Currency,
    /// How often to materialise.
    pub cadence: Cadence,
    /// First cycle date (inclusive).
    pub start_date: NaiveDate,
    /// Last cycle date (inclusive) — `None` = open-ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_date: Option<NaiveDate>,
    /// If true, materialised invoices auto-transition `draft → issued`.
    #[serde(default)]
    pub auto_issue: bool,
    /// Next planned materialisation date (advanced by the ticker).
    pub next_run: NaiveDate,
    /// Most-recent materialisation date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<NaiveDate>,
    /// Lifecycle.
    pub state: ScheduleState,
    /// Free-form metadata.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// DB insert time.
    pub created_at: DateTime<Utc>,
    /// Last modified.
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence_round_trip_monthly() {
        let c = Cadence::Monthly { dom: 15 };
        assert_eq!(c.to_string(), "monthly@15");
        assert_eq!("monthly@15".parse::<Cadence>().unwrap(), c);
    }

    #[test]
    fn cadence_round_trip_quarterly() {
        let c = Cadence::Quarterly { dom: 1 };
        assert_eq!(c.to_string(), "quarterly@1");
        assert_eq!("quarterly@1".parse::<Cadence>().unwrap(), c);
    }

    #[test]
    fn cadence_round_trip_yearly() {
        let c = Cadence::Yearly { month: 1, day: 1 };
        assert_eq!(c.to_string(), "yearly@01-01");
        assert_eq!("yearly@01-01".parse::<Cadence>().unwrap(), c);
    }

    #[test]
    fn cadence_rejects_invalid_dom() {
        assert!("monthly@0".parse::<Cadence>().is_err());
        assert!("monthly@29".parse::<Cadence>().is_err());
        assert!("monthly@abc".parse::<Cadence>().is_err());
    }

    #[test]
    fn cadence_rejects_missing_at() {
        assert!("monthly".parse::<Cadence>().is_err());
    }

    #[test]
    fn cadence_rejects_unknown_prefix() {
        assert!("weekly@1".parse::<Cadence>().is_err());
    }

    #[test]
    fn cadence_serde_through_string() {
        let c = Cadence::Monthly { dom: 1 };
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, r#""monthly@1""#);
        let back: Cadence = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn schedule_state_serde() {
        assert_eq!(serde_json::to_string(&ScheduleState::Active).unwrap(), r#""active""#);
        assert_eq!(serde_json::to_string(&ScheduleState::Paused).unwrap(), r#""paused""#);
    }
}
