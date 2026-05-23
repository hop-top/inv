//! Shared test fixtures for the e2e crate.
//!
//! Mirrors the patterns established in `crates/api/tests/integration.rs`
//! and `crates/commands/tests/commands.rs`:
//! - in-memory sqlite pool + migrations
//! - frozen-clock Clock impl
//! - InMemoryPublisher captured into a returned Arc for assertions
//! - QC tax-table fixture (+ DZ rows for the agency stories)
//!
//! Plus an `xrr_session` helper that defaults to Replay and switches to
//! Record when `XRR_MODE=record` is set.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};

use inv_api::{ApiConfig, ApiState};
use inv_bus::InMemoryPublisher;
use inv_commands::{Clock, CoreCtx};
use inv_core::domain::address::Address;
use inv_core::domain::customer::Customer;
use inv_core::domain::ids::CustomerId;
use inv_core::tax::TaxTable;
use inv_store::blob::LocalBlobStore;
use inv_store::pool::{connect, Pool};
use inv_store::repo::customer::CustomerRepo;
use inv_store::run_migrations;

use hop_top_xrr::{FileCassette, Mode, Session};

// =============================================================================
// Frozen clock
// =============================================================================

/// Frozen-clock Clock impl. Matches the QC/DZ stories' "today is X" framing.
pub struct FrozenClock(pub DateTime<Utc>);

impl Clock for FrozenClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// 2026-05-19T12:00:00Z. Matches the upstream test crates so blob /
/// invoice number sequences are deterministic across the workspace.
pub fn frozen_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap()
}

// =============================================================================
// Tax-table fixture
// =============================================================================

/// Inline tax-table that covers the three jurisdictions exercised by the
/// stories: CA-QC (freelancer-qc + saas-biller-de) and DZ-16 (agency-owner-dz).
/// Pulled from `tax-tables/default.toml` so the rate ids match what
/// `invoice_lines.tax_rate_ids` carries.
pub const FIXTURE_TOML: &str = r#"
[[rate]]
id = "ca-qc-gst"
jurisdiction = "CA-QC"
applies_to_buyer = { country = "CA" }
name = "GST"
category = "standard"
rate = "0.05"
effective_from = "2008-01-01"

[[rate]]
id = "ca-qc-qst"
jurisdiction = "CA-QC"
applies_to_buyer = { country = "CA", region = "QC" }
name = "QST"
category = "standard"
rate = "0.09975"
effective_from = "2013-01-01"

[[rate]]
id = "dz-tva-standard"
jurisdiction = "DZ-16"
applies_to_buyer = { country = "DZ" }
name = "TVA"
category = "standard"
rate = "0.19"
effective_from = "2017-01-01"

[[rate]]
id = "dz-tva-reduced"
jurisdiction = "DZ-16"
applies_to_buyer = { country = "DZ" }
name = "TVA reduite"
category = "reduced"
rate = "0.09"
effective_from = "2017-01-01"

[[rate]]
id = "dz-tva-export-zero"
jurisdiction = "DZ-16"
applies_to_buyer = { }
name = "TVA zero-rated (exports)"
category = "zero_rated"
rate = "0.00"
effective_from = "2017-01-01"
"#;

// =============================================================================
// CoreCtx + Pool fixtures
// =============================================================================

/// Build a fresh in-memory sqlite pool with migrations applied.
pub async fn fresh_pool() -> Pool {
    let pool = connect("sqlite::memory:").await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    pool
}

/// Build a fresh CoreCtx wired with: frozen clock, local blob store
/// (tempdir), and the InMemoryPublisher returned in the tuple so tests
/// can assert emitted bus events.
pub async fn fresh_ctx() -> (CoreCtx, Pool, Arc<InMemoryPublisher>, tempfile::TempDir) {
    let pool = fresh_pool().await;
    let (table, nexus) = TaxTable::load_from_str(FIXTURE_TOML).expect("tax fixture");
    let publisher: Arc<InMemoryPublisher> = Arc::new(InMemoryPublisher::new());
    let blob_dir = tempfile::tempdir().expect("blob tempdir");
    let blob_store =
        LocalBlobStore::new(blob_dir.path(), b"test-signing-key".to_vec()).expect("blob store");
    let ctx = CoreCtx::new(pool.clone(), table, nexus)
        .with_clock(Arc::new(FrozenClock(frozen_now())))
        .with_blob_store(Arc::new(blob_store))
        .with_publisher(publisher.clone() as Arc<dyn inv_commands::Publisher>);
    (ctx, pool, publisher, blob_dir)
}

/// Build a fresh ApiState + Pool + Publisher, leaving auth disabled and
/// wiring the same tax fixture + frozen clock as `fresh_ctx`.
pub async fn fresh_api_state() -> (
    Arc<ApiState>,
    Pool,
    Arc<InMemoryPublisher>,
    tempfile::TempDir,
) {
    let pool = fresh_pool().await;
    let (table, nexus) = TaxTable::load_from_str(FIXTURE_TOML).expect("tax fixture");
    let publisher: Arc<InMemoryPublisher> = Arc::new(InMemoryPublisher::new());
    let blob_dir = tempfile::tempdir().expect("blob tempdir");
    let blob_store =
        LocalBlobStore::new(blob_dir.path(), b"test-signing-key".to_vec()).expect("blob store");
    let ctx = CoreCtx::new(pool.clone(), table, nexus)
        .with_clock(Arc::new(FrozenClock(frozen_now())))
        .with_blob_store(Arc::new(blob_store))
        .with_publisher(publisher.clone() as Arc<dyn inv_commands::Publisher>);
    let state = Arc::new(ApiState::new(
        Arc::new(ctx),
        ApiConfig {
            link_signing_key: b"test-link-key".to_vec(),
            link_ttl: Duration::from_secs(3600),
            webhook_signing_key: b"test-webhook-key".to_vec(),
            public_base_url: "http://localhost:7400".into(),
            bearer_tokens: Vec::new(),
        },
    ));
    (state, pool, publisher, blob_dir)
}

// =============================================================================
// Customer seeding
// =============================================================================

/// Seed a QC customer matching the freelancer-qc + saas-biller-de stories.
pub async fn seed_customer_qc(pool: &Pool) -> CustomerId {
    seed_customer_with_address(
        pool,
        "Acme QC",
        Address {
            country: "CA".into(),
            region: Some("QC".into()),
            city: Some("Montréal".into()),
            postal: None,
            line1: None,
            line2: None,
        },
    )
    .await
}

/// Seed a DZ customer matching the agency-owner-dz local-TVA story.
pub async fn seed_customer_dz(pool: &Pool) -> CustomerId {
    seed_customer_with_address(
        pool,
        "Client DZ",
        Address {
            country: "DZ".into(),
            region: Some("16".into()),
            city: Some("Alger".into()),
            postal: None,
            line1: None,
            line2: None,
        },
    )
    .await
}

/// Seed a foreign-buyer customer (US) for the DZ export story. Per
/// stories/agency-owner-dz-02 the canonical buyer is FR/EUR, but
/// `inv-commands` rejects EUR at v1 (USD/CAD/DZD only). The buyer's
/// country alone determines `export` scope in the resolver — currency
/// doesn't gate it — so US/USD exercises the same code path with a
/// currency the validator accepts.
pub async fn seed_customer_foreign(pool: &Pool) -> CustomerId {
    seed_customer_with_address(
        pool,
        "Foreign Buyer",
        Address {
            country: "US".into(),
            region: Some("DE".into()),
            city: Some("Wilmington".into()),
            postal: None,
            line1: None,
            line2: None,
        },
    )
    .await
}

async fn seed_customer_with_address(pool: &Pool, name: &str, address: Address) -> CustomerId {
    let c = Customer {
        id: CustomerId::new(),
        display_name: name.to_string(),
        email: Some(format!(
            "billing@{}.example",
            name.to_ascii_lowercase().replace(' ', "-")
        )),
        address,
        metadata: BTreeMap::new(),
        created_at: frozen_now(),
        updated_at: frozen_now(),
    };
    CustomerRepo::new(pool)
        .save(&c)
        .await
        .expect("seed customer");
    c.id
}

// =============================================================================
// xrr session helper
// =============================================================================

/// Build an xrr session bound to `cassettes/<test_name>/`. Defaults to
/// Replay; set `XRR_MODE=record` to record. Per the project rule, the
/// cassette dir is workspace-relative — cargo runs tests from the crate
/// dir, so the path resolves relative to `crates/e2e/`.
pub fn xrr_session(test_name: &str) -> Session {
    let mode = match std::env::var("XRR_MODE").as_deref() {
        Ok("record") => Mode::Record,
        _ => Mode::Replay,
    };
    // `cassettes/<test>/` — relative to crate root (cargo cd's there).
    let cassette_dir = format!("cassettes/{test_name}");
    // Make sure record mode has a dir to write into; replay leaves the
    // dir untouched.
    if matches!(mode, Mode::Record) {
        std::fs::create_dir_all(&cassette_dir).expect("create cassette dir");
    }
    Session::new(mode, FileCassette::new(cassette_dir))
}
