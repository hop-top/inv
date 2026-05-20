//! [`CoreCtx`] — the dependency bundle every command consumes.
//!
//! Channels build a `CoreCtx` once, then hand it to every command. The
//! struct stays small at v1; later tasks (T-0014 bus, T-0009 blob,
//! T-0013 outbound channels) add fields without changing the command
//! signatures.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use inv_core::domain::invoice::HistoryChannel;
use inv_core::tax::{NexusConfig, TaxTable};
use inv_store::blob::BlobStore;
use inv_store::pool::Pool;

/// Trait so tests can inject a fixed clock.
///
/// Production wiring uses [`SystemClock`]; tests use a
/// `FrozenClock` that returns a constant `DateTime<Utc>`. The trait is
/// `Send + Sync` so a `CoreCtx` can be moved across tasks.
pub trait Clock: Send + Sync {
    /// Current wall-clock time.
    fn now(&self) -> DateTime<Utc>;
}

/// Default `Clock` impl using `chrono::Utc::now()`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Who (or what) triggered a command.
///
/// Channels supply the actor that the audit row records and the bus
/// payload carries. The `Bus { source }` variant captures the upstream
/// service id when a bus consumer (e.g. fin) triggered the action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// CLI user (typically `$USER`).
    Cli {
        /// Username / display string.
        name: String,
    },
    /// HTTP API user (whatever the auth layer resolved to).
    Api {
        /// Display string for the principal.
        name: String,
    },
    /// WebSocket session.
    Ws {
        /// Display string for the principal.
        name: String,
    },
    /// MCP agent.
    Mcp {
        /// Display string for the agent.
        name: String,
    },
    /// Inbound bus event consumer.
    Bus {
        /// Upstream service id (e.g. `"fin"`).
        source: String,
    },
}

impl Actor {
    /// Render the actor as the audit string stored on
    /// `invoice_state_history.actor`.
    pub fn audit_string(&self) -> String {
        match self {
            Actor::Cli { name } => name.clone(),
            Actor::Api { name } => name.clone(),
            Actor::Ws { name } => name.clone(),
            Actor::Mcp { name } => name.clone(),
            Actor::Bus { source } => format!("{source}.bus.consumer"),
        }
    }
}

/// Channel a command came in on. Mirrors
/// [`inv_core::domain::invoice::HistoryChannel`] one-for-one (kept here
/// so adapters can build a `CoreCtx` without re-importing the domain
/// module just for the enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// CLI adapter.
    Cli,
    /// HTTP API adapter.
    Api,
    /// WebSocket adapter.
    Ws,
    /// MCP adapter.
    Mcp,
    /// Bus consumer.
    Bus,
}

impl From<Channel> for HistoryChannel {
    fn from(c: Channel) -> Self {
        match c {
            Channel::Cli => HistoryChannel::Cli,
            Channel::Api => HistoryChannel::Api,
            Channel::Ws => HistoryChannel::Ws,
            Channel::Mcp => HistoryChannel::Mcp,
            Channel::Bus => HistoryChannel::Bus,
        }
    }
}

/// Bundle of dependencies that every command in this crate consumes.
///
/// `bus_publisher` is `None` at v1 — events are returned in the command
/// output for the test harness, and T-0014 will hook an actual
/// publisher behind a trait. `blob_store` is `Option` to keep the
/// dependency soft: when absent (e.g. lightweight test harnesses or
/// adapters that don't need PDF persistence), commands that render
/// content skip the persistence step and leave `pdf_blob_ref` unset.
#[derive(Clone)]
pub struct CoreCtx {
    /// Async sqlx pool the repos use.
    pub db: Pool,
    /// Loaded tax-table (per design §6.2 a single TOML file).
    pub tax_table: Arc<TaxTable>,
    /// US economic-nexus config (paired with `tax_table`).
    pub nexus: Arc<NexusConfig>,
    /// Injectable clock.
    pub clock: Arc<dyn Clock>,
    /// Optional blob backend used to persist rendered PDFs (T-0009).
    /// When `None`, commands skip blob persistence and leave the
    /// invoice's `pdf_blob_ref` field unchanged.
    pub blob_store: Option<Arc<dyn BlobStore>>,
}

impl CoreCtx {
    /// Construct with the default [`SystemClock`] and no blob store.
    pub fn new(db: Pool, tax_table: TaxTable, nexus: NexusConfig) -> Self {
        Self {
            db,
            tax_table: Arc::new(tax_table),
            nexus: Arc::new(nexus),
            clock: Arc::new(SystemClock),
            blob_store: None,
        }
    }

    /// Override the clock (used by tests to freeze time).
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Attach a blob store. Callers that want rendered PDFs to land in
    /// persistent storage (and `Invoice.pdf_blob_ref` to be set) must
    /// install one.
    pub fn with_blob_store(mut self, blob_store: Arc<dyn BlobStore>) -> Self {
        self.blob_store = Some(blob_store);
        self
    }
}

impl std::fmt::Debug for CoreCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreCtx")
            .field("db", &"<sqlx::AnyPool>")
            .field("tax_table_rows", &self.tax_table.len())
            .field("nexus_states", &self.nexus.states.len())
            .field("blob_store", &self.blob_store.is_some())
            .finish()
    }
}
