//! `inv tick …` — manual triggers for the background tickers.
//!
//! Mostly useful for tests + operator chores. Production runs invoke
//! the tickers from `inv-server` (T-0020). Each subcommand calls the
//! corresponding `inv-commands` function and renders a summary.

use anyhow::{anyhow, Context, Result};
use clap::{ArgMatches, Command};
use hop_top_kit::output::ColumnSpec;
use serde_json::json;

use inv_commands::{mark_overdue_ticker, reminders_tick, schedules_tick, CoreCtx};

use crate::render::render_value;

/// Build the `inv tick` subcommand tree.
pub fn command() -> Command {
    Command::new("tick")
        .about("Manually trigger a ticker (schedules | reminders | overdue)")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(Command::new("schedules").about("Run the schedules ticker once"))
        .subcommand(Command::new("reminders").about("Run the reminders ticker once"))
        .subcommand(Command::new("overdue").about("Run the overdue-marker ticker once"))
}

/// Dispatch `inv tick …`.
pub async fn dispatch(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    match matches.subcommand() {
        Some(("schedules", sub)) => run_schedules(ctx, sub).await,
        Some(("reminders", sub)) => run_reminders(ctx, sub).await,
        Some(("overdue", sub)) => run_overdue(ctx, sub).await,
        _ => Err(anyhow!("tick: missing subcommand")),
    }
}

async fn run_schedules(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let output = schedules_tick(ctx).await.context("schedules_tick failed")?;
    let value = json!({
        "ran_schedule_ids": output
            .ran_schedule_ids
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        "drafts": output.drafts.iter().map(|d| d.invoice.id.to_string()).collect::<Vec<_>>(),
    });
    render_value(matches, value, &tick_columns())?;
    Ok(())
}

async fn run_reminders(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let output = reminders_tick(ctx).await.context("reminders_tick failed")?;
    let value = json!({
        "sent_reminders": output
            .sent_reminders
            .iter()
            .map(|r| r.id.to_string())
            .collect::<Vec<_>>(),
    });
    render_value(matches, value, &tick_columns())?;
    Ok(())
}

async fn run_overdue(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let output = mark_overdue_ticker(ctx)
        .await
        .context("mark_overdue_ticker failed")?;
    let value = json!({
        "overdue_invoices": output
            .overdue_invoices
            .iter()
            .map(|i| i.id.to_string())
            .collect::<Vec<_>>(),
    });
    render_value(matches, value, &tick_columns())?;
    Ok(())
}

fn tick_columns() -> Vec<ColumnSpec> {
    vec![
        ColumnSpec::new("ran_schedule_ids", "ran_schedule_ids", 24),
        ColumnSpec::new("sent_reminders", "sent_reminders", 24),
        ColumnSpec::new("overdue_invoices", "overdue_invoices", 24),
        ColumnSpec::new("drafts", "drafts", 16),
    ]
}
