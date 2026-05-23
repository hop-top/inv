//! hop-top-inv-cli — CLI channel adapter.
//!
//! Library shape (shape-1 workspace): `bin/inv/` is the thin binary;
//! everything lives here. Decodes clap arguments → command inputs and
//! calls into `hop-top-inv-commands` for every operation in design §10. Output
//! flows through the kit `output` flag suite — every show/list as well
//! as single-row mutation outputs render via the same `dispatch`.

use std::path::PathBuf;

use clap::{Arg, ArgMatches, Command};
use hop_top_kit::output::{register_output_flags, RegisterOutputFlagsOptions};

mod commands;
mod config;
mod ctx;
mod render;

use config::Config;

/// Build the top-level clap command — exposed so unit tests can probe
/// the argument tree without running the async dispatch.
fn build_cli() -> Command {
    let cmd = Command::new("inv")
        .version(env!("CARGO_PKG_VERSION"))
        .about(
            "Invoicing-as-a-service: composer + lifecycle, multi-channel core, event-bus interop with fin",
        )
        .subcommand_required(true)
        .arg_required_else_help(true)
        .arg(
            Arg::new("config")
                .long("config")
                .help("Path to an inv.toml config file (default: $XDG_CONFIG_HOME/inv/config.toml)")
                .global(true),
        )
        .subcommand(commands::invoice::command())
        .subcommand(commands::creditnote::command())
        .subcommand(commands::schedule::command())
        .subcommand(commands::reminder::command())
        .subcommand(commands::customer::command())
        .subcommand(commands::tax::command())
        .subcommand(commands::tick::command())
        .subcommand(commands::server::command());

    let (cmd, _ctx) = register_output_flags(cmd, RegisterOutputFlagsOptions::default());
    cmd
}

/// CLI entrypoint. Called by `bin/inv/src/main.rs`.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let matches = build_cli().get_matches();
    // Build a tokio runtime here so the binary's `main()` stays sync
    // (matches the scaffold) and so we don't drag #[tokio::main] into
    // every callsite.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime
        .block_on(async_run(&matches))
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(AnyhowError(e)) })
}

/// Newtype wrapper to convert `anyhow::Error` into `Box<dyn Error>`
/// without dragging an extra dependency just for the conversion.
#[derive(Debug)]
struct AnyhowError(anyhow::Error);

impl std::fmt::Display for AnyhowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#}", self.0)
    }
}

impl std::error::Error for AnyhowError {}

async fn async_run(matches: &ArgMatches) -> anyhow::Result<()> {
    // Resolve config (or fall back to defaults).
    let config_path = matches.get_one::<String>("config").map(PathBuf::from);
    let cfg = Config::load(config_path.as_deref())?;
    let config_dir = config_path
        .as_ref()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    // Build CoreCtx (DB + tax table + clock).
    let ctx = ctx::build(cfg, config_dir).await?;

    // Dispatch on top-level subcommand.
    match matches.subcommand() {
        Some(("invoice", sub)) => commands::invoice::dispatch(&ctx, sub).await?,
        Some(("creditnote", sub)) => commands::creditnote::dispatch(&ctx, sub).await?,
        Some(("schedule", sub)) => commands::schedule::dispatch(&ctx, sub).await?,
        Some(("reminder", sub)) => commands::reminder::dispatch(&ctx, sub).await?,
        Some(("customer", sub)) => commands::customer::dispatch(&ctx, sub).await?,
        Some(("tax", sub)) => commands::tax::dispatch(&ctx, sub).await?,
        Some(("tick", sub)) => commands::tick::dispatch(&ctx, sub).await?,
        Some(("server", sub)) => commands::server::dispatch(&ctx, sub).await?,
        // `subcommand_required(true)` already rejects this path.
        _ => unreachable!("clap would have rejected an empty subcommand"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Unit tests for the arg-parsing → Input-struct mapping. These
    //! exercise [`build_cli`] + the matching `build_*_input` helpers
    //! WITHOUT touching the DB / tax-table pipeline.

    use rust_decimal::Decimal;
    use std::str::FromStr;

    use hop_top_inv_commands::{Actor, Channel};

    use super::build_cli;
    use crate::commands::invoice::{build_draft_input, build_issue_input, build_pay_input};

    fn parse(args: &[&str]) -> clap::ArgMatches {
        build_cli().try_get_matches_from(args).expect("parse")
    }

    fn actor() -> Actor {
        Actor::Cli {
            name: "tester".to_string(),
        }
    }

    #[test]
    fn draft_input_maps_clap_args() {
        let m = parse(&[
            "inv",
            "invoice",
            "draft",
            "--customer",
            "customer_01k2sd4d3rer7vd29crpz4nczt",
            "--currency",
            "CAD",
            "--seller-jurisdiction",
            "CA-QC",
            "--line",
            "Consulting:10:125.00",
            "--line",
            "Travel:1:200",
            "--idempotency-key",
            "key-1",
        ]);
        let sub = m
            .subcommand_matches("invoice")
            .and_then(|m| m.subcommand_matches("draft"))
            .expect("draft sub");
        let input = build_draft_input(sub, actor()).expect("input");
        assert_eq!(input.currency.to_string(), "CAD");
        assert_eq!(input.seller_jurisdiction.to_string(), "CA-QC");
        assert_eq!(input.lines.len(), 2);
        assert_eq!(input.lines[0].description, "Consulting");
        assert_eq!(input.lines[0].quantity, Decimal::from_str("10").unwrap());
        assert_eq!(
            input.lines[0].unit_price,
            Decimal::from_str("125.00").unwrap()
        );
        assert_eq!(input.lines[1].description, "Travel");
        assert_eq!(input.lines[1].unit_price, Decimal::from_str("200").unwrap());
        assert_eq!(input.idempotency_key.as_deref(), Some("key-1"));
        assert!(matches!(input.channel, Channel::Cli));
    }

    #[test]
    fn issue_input_maps_clap_args() {
        // We can't trivially mint a real typeid here, but the parser
        // does the typeid round-trip; reuse a freshly-minted one.
        use hop_top_inv_core::domain::ids::InvoiceId;
        let id = InvoiceId::new();
        let id_s = id.to_string();
        let m = parse(&["inv", "invoice", "issue", &id_s]);
        let sub = m
            .subcommand_matches("invoice")
            .and_then(|m| m.subcommand_matches("issue"))
            .expect("issue sub");
        let input = build_issue_input(sub, actor()).expect("input");
        assert_eq!(input.invoice_id.to_string(), id_s);
        assert!(matches!(input.channel, Channel::Cli));
    }

    #[test]
    fn pay_input_maps_clap_args() {
        use hop_top_inv_core::domain::ids::InvoiceId;
        let id = InvoiceId::new();
        let id_s = id.to_string();
        let m = parse(&[
            "inv",
            "invoice",
            "pay",
            &id_s,
            "--amount",
            "100.00",
            "--idempotency-key",
            "pay-1",
        ]);
        let sub = m
            .subcommand_matches("invoice")
            .and_then(|m| m.subcommand_matches("pay"))
            .expect("pay sub");
        let input = build_pay_input(sub, actor()).expect("input");
        assert_eq!(input.invoice_id.to_string(), id_s);
        assert_eq!(input.amount, Decimal::from_str("100.00").unwrap());
        assert_eq!(input.idempotency_key.as_deref(), Some("pay-1"));
    }
}
