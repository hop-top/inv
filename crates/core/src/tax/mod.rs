//! Tax engine: rate-table loader, US economic-nexus config, and the
//! `resolve_tax()` function that computes per-line tax for inv's three
//! seller jurisdictions and three markets.
//!
//! See the design spec, section 6, for the algorithm and TOML shape.
//! The engine is deliberately a *calculator* over an operator-supplied
//! rate table — not a tax-law oracle. Operators are responsible for
//! keeping `tax-tables/default.toml` (or any custom overlay) accurate.

pub mod nexus;
pub mod rate;
pub mod resolver;
pub mod table;

pub use nexus::{NexusConfig, NexusThreshold};
pub use rate::{AppliesToBuyer, TaxRate};
pub use resolver::{
    resolve_tax, BuyerScope, NexusFigures, ResolvedTax, TaxError,
};
pub use table::{TaxFile, TaxTable, TaxTableError};
