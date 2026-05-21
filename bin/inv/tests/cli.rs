//! Integration smoke tests for the `inv` binary.
//!
//! Each test runs the binary against a temp config that points at an
//! in-memory sqlite DB (the default fallback) and an explicit
//! `tax-tables/default.toml` so it works from any CWD. We assert on
//! exit status + a few stable output fragments — full surface coverage
//! lives in the per-subcommand unit tests inside `inv-cli`.

use assert_cmd::Command;
use predicates::prelude::*;

/// Path to the bundled tax table, resolved from the workspace root via
/// `CARGO_MANIFEST_DIR` (`bin/inv/`). Two levels up brings us to the
/// workspace root that owns `tax-tables/`.
fn tax_tables_path() -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    std::path::Path::new(manifest)
        .join("..")
        .join("..")
        .join("tax-tables/default.toml")
        .canonicalize()
        .expect("tax-tables/default.toml resolvable from bin/inv/")
        .display()
        .to_string()
}

/// Wrap `Command::cargo_bin("inv")` and set INV_TAX_TABLES so the CLI
/// finds the bundled tax table without a config file.
fn inv() -> Command {
    let mut cmd = Command::cargo_bin("inv").expect("inv binary built");
    cmd.env("INV_TAX_TABLES", tax_tables_path());
    cmd
}

#[test]
fn invoice_list_empty_db_returns_empty_json_array() {
    inv()
        .args(["invoice", "list", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::eq("[]\n").or(predicate::eq("[]")));
}

#[test]
fn tax_rates_show_yaml_has_rows() {
    inv()
        .args(["tax", "rates", "show", "--format", "yaml"])
        .assert()
        .success()
        // At minimum the bundled table includes the GST/QST rows; we
        // check for a known stable substring.
        .stdout(predicate::str::contains("jurisdiction"));
}

#[test]
fn invoice_list_table_format_default() {
    // Default --format=table on an empty DB is still a success exit;
    // table output may be a one-liner or empty depending on schema.
    inv()
        .args(["invoice", "list"])
        .assert()
        .success();
}
