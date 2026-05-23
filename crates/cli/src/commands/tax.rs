//! `inv tax …` subcommand tree.
//!
//! Only one read-only operation at v1: `inv tax rates show` dumps the
//! loaded `TaxTable` to the kit output dispatch (json/yaml/table). The
//! rates come from whichever file was loaded into the `CoreCtx` at
//! startup — see [`crate::ctx::build`].

use anyhow::{anyhow, Result};
use clap::{ArgMatches, Command};
use hop_top_kit::output::ColumnSpec;
use serde_json::json;

use hop_top_inv_commands::CoreCtx;

use crate::render::render_list;

/// Build the `inv tax` subcommand tree.
pub fn command() -> Command {
    Command::new("tax")
        .about("Tax engine: inspect the loaded rate table")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("rates")
                .about("Tax rates")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(Command::new("show").about("Dump the loaded tax table")),
        )
}

/// Dispatch `inv tax …`.
pub async fn dispatch(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    match matches.subcommand() {
        Some(("rates", sub)) => match sub.subcommand() {
            Some(("show", show)) => run_rates_show(ctx, show).await,
            _ => Err(anyhow!("tax rates: missing subcommand")),
        },
        _ => Err(anyhow!("tax: missing subcommand")),
    }
}

async fn run_rates_show(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    // The table's rates field exposes the loaded rows. We render them
    // as a JSON array via the kit dispatch.
    let rows = json!(ctx
        .tax_table
        .rates
        .iter()
        .map(|r| {
            json!({
                "id": r.id,
                "jurisdiction": r.jurisdiction.to_string(),
                "name": r.name,
                "category": format!("{:?}", r.category).to_lowercase(),
                "rate": r.rate.to_string(),
                "country": r.applies_country(),
                "region": r.applies_region(),
                "effective_from": r.effective_from.to_string(),
                "effective_to": r.effective_to.map(|d| d.to_string()),
            })
        })
        .collect::<Vec<_>>());
    render_list(matches, rows, &columns())?;
    Ok(())
}

fn columns() -> Vec<ColumnSpec> {
    vec![
        ColumnSpec::new("id", "id", 16),
        ColumnSpec::new("jurisdiction", "jurisdiction", 10),
        ColumnSpec::new("name", "name", 8),
        ColumnSpec::new("category", "category", 10),
        ColumnSpec::new("rate", "rate", 8),
    ]
}
