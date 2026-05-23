//! Argument-parsing helpers shared across subcommand handlers.

use std::str::FromStr;

use anyhow::{anyhow, Result};
use chrono::{DateTime, NaiveDate, Utc};
use rust_decimal::Decimal;

use inv_core::domain::ids::{CreditNoteId, CustomerId, InvoiceId, ReminderId, ScheduleId};

/// Parse a decimal amount; surfaces a friendly error on failure.
pub fn parse_decimal(label: &str, raw: &str) -> Result<Decimal> {
    Decimal::from_str(raw).map_err(|e| anyhow!("invalid {label} `{raw}`: {e}"))
}

/// Parse an RFC 3339 datetime.
pub fn parse_datetime(label: &str, raw: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| anyhow!("invalid {label} `{raw}` (expected RFC 3339): {e}"))
}

/// Parse a `YYYY-MM-DD` date.
pub fn parse_date(label: &str, raw: &str) -> Result<NaiveDate> {
    NaiveDate::from_str(raw)
        .map_err(|e| anyhow!("invalid {label} `{raw}` (expected YYYY-MM-DD): {e}"))
}

/// Parse a typeid string into the matching newtype.
pub fn parse_invoice_id(raw: &str) -> Result<InvoiceId> {
    raw.parse::<InvoiceId>()
        .map_err(|e| anyhow!("invalid invoice id `{raw}`: {e}"))
}

/// Parse a customer typeid.
pub fn parse_customer_id(raw: &str) -> Result<CustomerId> {
    raw.parse::<CustomerId>()
        .map_err(|e| anyhow!("invalid customer id `{raw}`: {e}"))
}

/// Parse a credit-note typeid.
pub fn parse_credit_note_id(raw: &str) -> Result<CreditNoteId> {
    raw.parse::<CreditNoteId>()
        .map_err(|e| anyhow!("invalid credit-note id `{raw}`: {e}"))
}

/// Parse a schedule typeid.
pub fn parse_schedule_id(raw: &str) -> Result<ScheduleId> {
    raw.parse::<ScheduleId>()
        .map_err(|e| anyhow!("invalid schedule id `{raw}`: {e}"))
}

/// Parse a reminder typeid.
pub fn parse_reminder_id(raw: &str) -> Result<ReminderId> {
    raw.parse::<ReminderId>()
        .map_err(|e| anyhow!("invalid reminder id `{raw}`: {e}"))
}

/// Parse a `<desc>:<qty>:<price>` line spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSpec {
    /// Free-form description.
    pub description: String,
    /// Quantity (positive Decimal).
    pub quantity: Decimal,
    /// Unit price (>= 0 Decimal).
    pub unit_price: Decimal,
}

/// Parse a `--line "<desc>:<qty>:<price>"` value.
///
/// The description is everything up to the last two colons so descriptions
/// containing single colons (`"Apt 1:200"`) still parse cleanly.
pub fn parse_line(raw: &str) -> Result<LineSpec> {
    // Split from the right: the trailing two segments are the numeric
    // qty + price; everything else is the description.
    let rsplit: Vec<&str> = raw.rsplitn(3, ':').collect();
    if rsplit.len() != 3 {
        return Err(anyhow!(
            "invalid --line `{raw}` (expected `<desc>:<qty>:<price>`)"
        ));
    }
    // rsplit returns [price, qty, desc]
    let unit_price = parse_decimal("line unit_price", rsplit[0])?;
    let quantity = parse_decimal("line quantity", rsplit[1])?;
    let description = rsplit[2].to_string();
    if description.trim().is_empty() {
        return Err(anyhow!("--line `{raw}`: description must not be empty"));
    }
    Ok(LineSpec {
        description,
        quantity,
        unit_price,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_line_simple() {
        let spec = parse_line("Consulting:10:125.00").unwrap();
        assert_eq!(spec.description, "Consulting");
        assert_eq!(spec.quantity, Decimal::from_str("10").unwrap());
        assert_eq!(spec.unit_price, Decimal::from_str("125.00").unwrap());
    }

    #[test]
    fn parse_line_description_with_colon() {
        let spec = parse_line("Apt 1:second floor:2:50").unwrap();
        assert_eq!(spec.description, "Apt 1:second floor");
        assert_eq!(spec.quantity, Decimal::from_str("2").unwrap());
        assert_eq!(spec.unit_price, Decimal::from_str("50").unwrap());
    }

    #[test]
    fn parse_line_rejects_missing_fields() {
        assert!(parse_line("just-desc").is_err());
        assert!(parse_line("desc:10").is_err());
    }
}
