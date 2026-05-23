//! `inv customer …` subcommand tree.
//!
//! Customers don't have a dedicated `hop-top-inv-commands` function — they're
//! treated as external references (design §5). The CLI wires
//! `CustomerRepo` directly so operators can seed customers without
//! invoking a sister service. When `hop-top-inv-commands` later grows a
//! `customer_create` command (e.g. for bus-driven onboarding) this
//! module collapses to a thin wrapper around it.

use std::collections::BTreeMap;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use clap::{value_parser, Arg, ArgMatches, Command};
use hop_top_kit::output::ColumnSpec;

use hop_top_inv_commands::CoreCtx;
use hop_top_inv_core::domain::address::Address;
use hop_top_inv_core::domain::customer::Customer;
use hop_top_inv_core::domain::ids::CustomerId;
use hop_top_inv_store::repo::customer::CustomerRepo;

use super::parse::parse_customer_id;
use crate::render::{render_list, render_value};

/// Build the `inv customer` subcommand tree.
pub fn command() -> Command {
    Command::new("customer")
        .about("Customers: add, show, list")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(add_cmd())
        .subcommand(
            Command::new("show").about("Show a single customer").arg(
                Arg::new("id")
                    .help("Customer typeid")
                    .required(true)
                    .index(1),
            ),
        )
        .subcommand(
            Command::new("list")
                .about("List customers")
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .value_parser(value_parser!(i64))
                        .default_value("100"),
                )
                .arg(
                    Arg::new("offset")
                        .long("offset")
                        .value_parser(value_parser!(i64))
                        .default_value("0"),
                ),
        )
}

fn add_cmd() -> Command {
    Command::new("add")
        .about("Add a new customer")
        .arg(
            Arg::new("name")
                .long("name")
                .help("Display name (rendered on invoices)")
                .required(true),
        )
        .arg(Arg::new("email").long("email").help("Optional email"))
        .arg(
            Arg::new("country")
                .long("country")
                .help("ISO 3166-1 alpha-2 country code")
                .required(true),
        )
        .arg(
            Arg::new("region")
                .long("region")
                .help("ISO 3166-2 subdivision (e.g. QC)"),
        )
        .arg(Arg::new("city").long("city").help("City name"))
        .arg(Arg::new("postal").long("postal").help("Postal code"))
        .arg(Arg::new("line1").long("line1").help("Address line 1"))
        .arg(Arg::new("line2").long("line2").help("Address line 2"))
}

/// Dispatch `inv customer …`.
pub async fn dispatch(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    match matches.subcommand() {
        Some(("add", sub)) => run_add(ctx, sub).await,
        Some(("show", sub)) => run_show(ctx, sub).await,
        Some(("list", sub)) => run_list(ctx, sub).await,
        _ => Err(anyhow!("customer: missing subcommand")),
    }
}

async fn run_add(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let display_name = matches
        .get_one::<String>("name")
        .ok_or_else(|| anyhow!("--name required"))?
        .clone();
    let email = matches.get_one::<String>("email").cloned();
    let country = matches
        .get_one::<String>("country")
        .ok_or_else(|| anyhow!("--country required"))?
        .clone();
    let address = Address {
        country,
        region: matches.get_one::<String>("region").cloned(),
        city: matches.get_one::<String>("city").cloned(),
        postal: matches.get_one::<String>("postal").cloned(),
        line1: matches.get_one::<String>("line1").cloned(),
        line2: matches.get_one::<String>("line2").cloned(),
    };

    let now = Utc::now();
    let customer = Customer {
        id: CustomerId::new(),
        display_name,
        email,
        address,
        metadata: BTreeMap::new(),
        created_at: now,
        updated_at: now,
    };

    CustomerRepo::new(&ctx.db)
        .save(&customer)
        .await
        .context("save customer")?;
    render_value(matches, serde_json::to_value(&customer)?, &columns())?;
    Ok(())
}

async fn run_show(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let id_raw = matches
        .get_one::<String>("id")
        .ok_or_else(|| anyhow!("customer id required"))?;
    let customer_id = parse_customer_id(id_raw)?;
    let customer = CustomerRepo::new(&ctx.db)
        .get(&customer_id)
        .await?
        .ok_or_else(|| anyhow!("customer {customer_id} not found"))?;
    render_value(matches, serde_json::to_value(customer)?, &columns())?;
    Ok(())
}

async fn run_list(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let limit = matches.get_one::<i64>("limit").copied().unwrap_or(100);
    let offset = matches.get_one::<i64>("offset").copied().unwrap_or(0);
    let rows = CustomerRepo::new(&ctx.db).list(limit, offset).await?;
    render_list(matches, serde_json::to_value(rows)?, &columns())?;
    Ok(())
}

fn columns() -> Vec<ColumnSpec> {
    vec![
        ColumnSpec::new("id", "id", 26),
        ColumnSpec::new("display_name", "display_name", 20),
        ColumnSpec::new("email", "email", 24),
    ]
}
