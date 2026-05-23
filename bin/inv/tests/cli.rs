//! Integration smoke tests for the `inv` binary.
//!
//! Each test runs the binary against a temp config that points at an
//! in-memory sqlite DB (the default fallback) and an explicit
//! `tax-tables/default.toml` so it works from any CWD. We assert on
//! exit status + a few stable output fragments — full surface coverage
//! lives in the per-subcommand unit tests inside `hop-top-inv-cli`.

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
    inv().args(["invoice", "list"]).assert().success();
}

/// Smoke test: `inv server` boots the HTTP listener, serves /healthz,
/// and exits cleanly on SIGTERM (or ctrl_c on Windows).
///
/// We spawn the binary as a subprocess so the long-lived tokio
/// runtime doesn't bleed into the test process; bind to a random
/// high port; poll healthz; signal; wait for exit.
#[test]
#[cfg(unix)]
fn server_starts_and_serves_healthz() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    // Pick an unused port by binding briefly then releasing.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
        listener.local_addr().unwrap().port()
    };
    let listen = format!("127.0.0.1:{port}");

    let manifest = env!("CARGO_MANIFEST_DIR");
    let tax = std::path::Path::new(manifest)
        .join("..")
        .join("..")
        .join("tax-tables/default.toml")
        .canonicalize()
        .expect("tax path");
    let bin = assert_cmd::cargo::cargo_bin("inv");

    let mut child = Command::new(&bin)
        .args(["server", "--listen", &listen])
        .env("INV_TAX_TABLES", tax)
        .env("RUST_LOG", "warn")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn inv server");

    // Drain stdout/stderr in background so the child doesn't block on
    // a full pipe. We don't assert on contents — just keep the pipe open.
    if let Some(stdout) = child.stdout.take() {
        thread::spawn(
            move || {
                for _ in BufReader::new(stdout).lines().map_while(Result::ok) {}
            },
        );
    }
    if let Some(stderr) = child.stderr.take() {
        thread::spawn(
            move || {
                for _ in BufReader::new(stderr).lines().map_while(Result::ok) {}
            },
        );
    }

    // Poll /healthz for up to ~5s.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ok = false;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            // Port open; try an HTTP GET via std-only.
            if let Ok(mut stream) = std::net::TcpStream::connect(format!("127.0.0.1:{port}")) {
                use std::io::{Read, Write};
                let req =
                    format!("GET /healthz HTTP/1.1\r\nHost: {listen}\r\nConnection: close\r\n\r\n");
                if stream.write_all(req.as_bytes()).is_ok() {
                    let mut buf = String::new();
                    let _ = stream.read_to_string(&mut buf);
                    if buf.starts_with("HTTP/1.1 200") {
                        ok = true;
                        break;
                    }
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }

    // SIGTERM → graceful exit (we abort tasks but axum drops cleanly).
    // Shell out to /bin/kill to avoid pulling `libc` + unsafe into the test.
    let _ = Command::new("/bin/kill")
        .args(["-TERM", &child.id().to_string()])
        .status();
    let _ = child.wait_timeout(Duration::from_secs(3));

    assert!(ok, "GET /healthz did not return 200 within 5s");
}

/// Tiny extension on Child::wait that times out instead of hanging
/// forever. We use it to bound test runtime if SIGTERM is ignored.
trait WaitTimeout {
    fn wait_timeout(
        &mut self,
        dur: std::time::Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>>;
}
impl WaitTimeout for std::process::Child {
    fn wait_timeout(
        &mut self,
        dur: std::time::Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>> {
        let start = std::time::Instant::now();
        while start.elapsed() < dur {
            if let Some(status) = self.try_wait()? {
                return Ok(Some(status));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // Force kill if it didn't exit on SIGTERM.
        let _ = self.kill();
        Ok(None)
    }
}
