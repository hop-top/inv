//! Subcommand handlers — one module per top-level noun, each exposing
//! a `command()` clap builder and a `dispatch` entrypoint that wires the
//! parsed `ArgMatches` to the matching `inv-commands` call.
//!
//! Common parsing helpers (decimal, datetime, id) live in [`parse`]; the
//! kit output dispatch lives in [`crate::render`].

pub mod creditnote;
pub mod customer;
pub mod invoice;
pub mod parse;
pub mod reminder;
pub mod schedule;
pub mod server;
pub mod tax;
pub mod tick;
