//! hop-top-inv-store — persistence: sql connection management, migrations,
//! and repository structs.
//!
//! Wires `sqlx` into the inv workspace. Backends are gated by Cargo
//! features (`sqlite` default; `postgres`; `tidb`); at least one must
//! be enabled.

#[cfg(not(any(feature = "sqlite", feature = "postgres", feature = "tidb")))]
compile_error!(
    "hop-top-inv-store requires at least one storage backend feature: `sqlite`, `postgres`, or `tidb`"
);

pub mod blob;
pub mod error;
pub mod migrate;
pub mod pool;
pub mod repo;

pub use error::{Result, StoreError};
pub use migrate::run_migrations;
pub use pool::{connect, Pool};
