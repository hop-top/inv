//! `inv reminder …` subcommand tree.

use anyhow::{anyhow, Context, Result};
use clap::{Arg, ArgMatches, Command};
use hop_top_kit::output::ColumnSpec;
use serde_json::json;

use inv_commands::{
    reminder_cancel, reminder_schedule, Actor, Channel, CoreCtx, ReminderCancelInput,
    ReminderScheduleInput,
};
use inv_core::domain::reminder::ReminderChannel;
use inv_store::repo::reminder::ReminderRepo;

use super::parse::{parse_datetime, parse_invoice_id, parse_reminder_id};
use crate::render::{render_list, render_value};

fn cli_actor() -> Actor {
    Actor::Cli {
        name: std::env::var("USER").unwrap_or_else(|_| "cli".to_string()),
    }
}

/// Build the `inv reminder` subcommand tree.
pub fn command() -> Command {
    Command::new("reminder")
        .about("Reminders: schedule, cancel, list")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("schedule")
                .about("Enqueue a reminder against an invoice")
                .arg(
                    Arg::new("invoice")
                        .long("invoice")
                        .help("Invoice typeid")
                        .required(true),
                )
                .arg(
                    Arg::new("at")
                        .long("at")
                        .help("Dispatch time (RFC 3339 datetime, must be in the future)")
                        .required(true),
                )
                .arg(
                    Arg::new("channel")
                        .long("channel")
                        .help("Delivery channel (file|stdout|bus|webhook|link)")
                        .required(true),
                ),
        )
        .subcommand(
            Command::new("cancel")
                .about("Cancel a scheduled reminder")
                .arg(
                    Arg::new("id")
                        .help("Reminder typeid")
                        .required(true)
                        .index(1),
                ),
        )
        .subcommand(
            Command::new("list")
                .about("List reminders for an invoice")
                .arg(
                    Arg::new("invoice")
                        .long("invoice")
                        .help("Invoice typeid")
                        .required(true),
                ),
        )
}

/// Dispatch `inv reminder …`.
pub async fn dispatch(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    match matches.subcommand() {
        Some(("schedule", sub)) => run_schedule(ctx, sub).await,
        Some(("cancel", sub)) => run_cancel(ctx, sub).await,
        Some(("list", sub)) => run_list(ctx, sub).await,
        _ => Err(anyhow!("reminder: missing subcommand")),
    }
}

async fn run_schedule(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let invoice_raw = matches
        .get_one::<String>("invoice")
        .ok_or_else(|| anyhow!("--invoice required"))?;
    let invoice_id = parse_invoice_id(invoice_raw)?;
    let at_raw = matches
        .get_one::<String>("at")
        .ok_or_else(|| anyhow!("--at required"))?;
    let scheduled_at = parse_datetime("--at", at_raw)?;
    let channel_raw = matches
        .get_one::<String>("channel")
        .ok_or_else(|| anyhow!("--channel required"))?;
    let channel_scheme = parse_channel(channel_raw)?;

    let input = ReminderScheduleInput {
        invoice_id,
        scheduled_at,
        channel_scheme,
        actor: cli_actor(),
        channel: Channel::Cli,
    };
    let output = reminder_schedule(ctx, input)
        .await
        .context("reminder_schedule failed")?;
    let value = json!({
        "reminder": output.reminder,
    });
    render_value(matches, value, &columns())?;
    Ok(())
}

async fn run_cancel(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let id_raw = matches
        .get_one::<String>("id")
        .ok_or_else(|| anyhow!("reminder id required"))?;
    let reminder_id = parse_reminder_id(id_raw)?;
    let input = ReminderCancelInput {
        reminder_id,
        actor: cli_actor(),
        channel: Channel::Cli,
    };
    let output = reminder_cancel(ctx, input)
        .await
        .context("reminder_cancel failed")?;
    let value = json!({
        "reminder": output.reminder,
    });
    render_value(matches, value, &columns())?;
    Ok(())
}

async fn run_list(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let invoice_raw = matches
        .get_one::<String>("invoice")
        .ok_or_else(|| anyhow!("--invoice required"))?;
    let invoice_id = parse_invoice_id(invoice_raw)?;
    let rows = ReminderRepo::new(&ctx.db)
        .list_for_invoice(&invoice_id)
        .await?;
    render_list(matches, serde_json::to_value(rows)?, &columns())?;
    Ok(())
}

fn parse_channel(raw: &str) -> Result<ReminderChannel> {
    Ok(match raw {
        "file" => ReminderChannel::File,
        "stdout" => ReminderChannel::Stdout,
        "bus" => ReminderChannel::Bus,
        "webhook" => ReminderChannel::Webhook,
        "link" => ReminderChannel::Link,
        other => return Err(anyhow!("unknown reminder channel `{other}`")),
    })
}

fn columns() -> Vec<ColumnSpec> {
    vec![
        ColumnSpec::new("id", "id", 26),
        ColumnSpec::new("invoice_id", "invoice_id", 26),
        ColumnSpec::new("scheduled_at", "scheduled_at", 24),
        ColumnSpec::new("channel", "channel", 10),
        ColumnSpec::new("state", "state", 10),
    ]
}
