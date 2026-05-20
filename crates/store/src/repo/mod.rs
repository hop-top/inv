//! Repository structs — async CRUD against the inv schema.
//!
//! Every repo takes `&Pool` (sqlx's `AnyPool`) and exposes `save() /
//! get() / list()` (+ `delete` where appropriate). Reads + writes go
//! through runtime `query()` rather than the macro form: `sqlx::query!`
//! requires a concrete backend at compile time, which conflicts with
//! the workspace's `AnyPool`-based wiring.

pub mod bus_inbox;
pub mod credit_note;
pub mod customer;
pub mod history;
pub mod invoice;
pub mod reminder;
pub mod schedule;

pub use bus_inbox::{BusInboxRecord, BusInboxRepo};
pub use credit_note::{CreditNoteHistoryRepo, CreditNoteRepo};
pub use customer::CustomerRepo;
pub use history::InvoiceHistoryRepo;
pub use invoice::{InvoiceLineRepo, InvoiceRepo};
pub use reminder::ReminderRepo;
pub use schedule::ScheduleRepo;

// ---------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use std::str::FromStr;

use crate::error::{Result, StoreError};

/// Parse a Decimal stored as TEXT (sqlite) or fetched as a Decimal-typed
/// value (postgres). Since we go through sqlx::Any and bind everything
/// as String, every row returns a String column for Decimal fields. The
/// sqlite backend stores them as TEXT verbatim; for postgres we keep
/// the columns NUMERIC at the schema level but bind/read via string
/// (sqlx's Any abstraction over Decimal binds to TEXT internally — see
/// pool.rs notes).
pub(crate) fn parse_decimal(s: &str) -> Result<Decimal> {
    Decimal::from_str(s).map_err(StoreError::from)
}

/// Render a Decimal in the canonical form we store.
pub(crate) fn decimal_to_string(d: &Decimal) -> String {
    d.to_string()
}

/// Convert an optional rfc3339-ish string column to `DateTime<Utc>`.
pub(crate) fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| StoreError::Other(format!("timestamp parse: {e}")))
}

/// Encode a `DateTime<Utc>` to an rfc3339 string for storage.
pub(crate) fn ts_to_string(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

/// Encode metadata (BTreeMap<String,String>) to JSON.
pub(crate) fn metadata_to_json(
    m: &std::collections::BTreeMap<String, String>,
) -> Result<String> {
    serde_json::to_string(m).map_err(StoreError::from)
}

/// Decode metadata from JSON; absent / empty string returns an empty map.
pub(crate) fn metadata_from_json(
    s: Option<&str>,
) -> Result<std::collections::BTreeMap<String, String>> {
    match s {
        None => Ok(Default::default()),
        Some("") => Ok(Default::default()),
        Some(text) => serde_json::from_str(text).map_err(StoreError::from),
    }
}
