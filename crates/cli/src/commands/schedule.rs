//! `inv schedule …` subcommand tree.

use anyhow::{anyhow, Context, Result};
use clap::{Arg, ArgAction, ArgMatches, Command};
use hop_top_kit::output::ColumnSpec;
use serde_json::{json, Value};

use inv_commands::{
    schedule_cancel, schedule_create, schedule_pause, Actor, Channel, CoreCtx,
    ScheduleCreateInput, ScheduleLineInput, ScheduleStateChangeInput,
};
use inv_core::domain::invoice::TaxCategory;
use inv_core::domain::money::Currency;
use inv_core::domain::schedule::{Cadence, ScheduleState};
use inv_store::repo::schedule::{ScheduleFilter, ScheduleRepo};

use super::parse::{
    parse_customer_id, parse_date, parse_line, parse_schedule_id, LineSpec,
};
use crate::render::{render_list, render_value};

fn cli_actor() -> Actor {
    Actor::Cli {
        name: std::env::var("USER").unwrap_or_else(|_| "cli".to_string()),
    }
}

/// Build the `inv schedule` clap subcommand.
pub fn command() -> Command {
    Command::new("schedule")
        .about("Recurring schedules: create, pause, cancel, show, list")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(create_cmd())
        .subcommand(
            Command::new("pause")
                .about("Pause an active schedule")
                .arg(Arg::new("id").help("Schedule typeid").required(true).index(1)),
        )
        .subcommand(
            Command::new("cancel")
                .about("Cancel a schedule (terminal)")
                .arg(Arg::new("id").help("Schedule typeid").required(true).index(1)),
        )
        .subcommand(
            Command::new("show")
                .about("Show a single schedule")
                .arg(Arg::new("id").help("Schedule typeid").required(true).index(1)),
        )
        .subcommand(
            Command::new("list")
                .about("List schedules (optionally filtered)")
                .arg(
                    Arg::new("customer")
                        .long("customer")
                        .help("Filter by customer typeid"),
                )
                .arg(
                    Arg::new("state")
                        .long("state")
                        .help("Filter by lifecycle state (active | paused | cancelled)"),
                )
                .arg(
                    Arg::new("limit")
                        .long("limit")
                        .help("Max rows"),
                )
                .arg(
                    Arg::new("offset")
                        .long("offset")
                        .help("Offset for paging"),
                ),
        )
}

fn create_cmd() -> Command {
    Command::new("create")
        .about("Create an active recurring schedule")
        .arg(
            Arg::new("customer")
                .long("customer")
                .help("Customer typeid")
                .required(true),
        )
        .arg(
            Arg::new("currency")
                .long("currency")
                .help("Invoice currency (USD | CAD | DZD)")
                .required(true),
        )
        .arg(
            Arg::new("cadence")
                .long("cadence")
                .help("Cadence spec (monthly@N | quarterly@N | yearly@MM-DD)")
                .required(true),
        )
        .arg(
            Arg::new("start")
                .long("start")
                .help("Start date (YYYY-MM-DD)")
                .required(true),
        )
        .arg(
            Arg::new("end")
                .long("end")
                .help("End date (YYYY-MM-DD, optional)"),
        )
        .arg(
            Arg::new("auto-issue")
                .long("auto-issue")
                .help("Auto-issue drafts on every cycle")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("line")
                .long("line")
                .help("Line template `<desc>:<qty>:<price>` (repeatable; at least one required)")
                .action(ArgAction::Append)
                .required(true),
        )
}

/// Dispatch `inv schedule …`.
pub async fn dispatch(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    match matches.subcommand() {
        Some(("create", sub)) => run_create(ctx, sub).await,
        Some(("pause", sub)) => run_pause(ctx, sub).await,
        Some(("cancel", sub)) => run_cancel(ctx, sub).await,
        Some(("show", sub)) => run_show(ctx, sub).await,
        Some(("list", sub)) => run_list(ctx, sub).await,
        _ => Err(anyhow!("schedule: missing subcommand")),
    }
}

async fn run_create(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let customer_raw = matches
        .get_one::<String>("customer")
        .ok_or_else(|| anyhow!("--customer required"))?;
    let customer_id = parse_customer_id(customer_raw)?;

    let currency_raw = matches
        .get_one::<String>("currency")
        .ok_or_else(|| anyhow!("--currency required"))?;
    let currency: Currency = currency_raw
        .parse()
        .map_err(|e| anyhow!("invalid --currency `{currency_raw}`: {e}"))?;

    let cadence_raw = matches
        .get_one::<String>("cadence")
        .ok_or_else(|| anyhow!("--cadence required"))?;
    let cadence: Cadence = cadence_raw
        .parse()
        .map_err(|e| anyhow!("invalid --cadence `{cadence_raw}`: {e}"))?;

    let start_raw = matches
        .get_one::<String>("start")
        .ok_or_else(|| anyhow!("--start required"))?;
    let start_date = parse_date("--start", start_raw)?;

    let end_date = match matches.get_one::<String>("end") {
        None => None,
        Some(raw) => Some(parse_date("--end", raw)?),
    };
    let auto_issue = matches.get_flag("auto-issue");

    let line_specs: Vec<LineSpec> = matches
        .get_many::<String>("line")
        .map(|it| it.map(|s| parse_line(s)).collect::<Result<Vec<_>>>())
        .transpose()?
        .unwrap_or_default();
    if line_specs.is_empty() {
        return Err(anyhow!("at least one --line is required"));
    }
    let template_lines: Vec<ScheduleLineInput> = line_specs
        .into_iter()
        .map(|l| ScheduleLineInput {
            description: l.description,
            quantity: l.quantity,
            unit_price: l.unit_price,
            tax_category: TaxCategory::Standard,
        })
        .collect();

    let input = ScheduleCreateInput {
        customer_id,
        template_lines,
        currency,
        cadence,
        start_date,
        end_date,
        auto_issue,
        actor: cli_actor(),
        channel: Channel::Cli,
    };
    let output = schedule_create(ctx, input)
        .await
        .context("schedule_create failed")?;
    let value = json!({
        "schedule": output.schedule,
        "emitted_events": event_topics(&output.emitted_events),
    });
    render_value(matches, value, &columns())?;
    Ok(())
}

async fn run_pause(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let input = build_state_change_input(matches)?;
    let output = schedule_pause(ctx, input)
        .await
        .context("schedule_pause failed")?;
    render_transition(matches, output)
}

async fn run_cancel(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let input = build_state_change_input(matches)?;
    let output = schedule_cancel(ctx, input)
        .await
        .context("schedule_cancel failed")?;
    render_transition(matches, output)
}

fn build_state_change_input(matches: &ArgMatches) -> Result<ScheduleStateChangeInput> {
    let id_raw = matches
        .get_one::<String>("id")
        .ok_or_else(|| anyhow!("schedule id required"))?;
    let schedule_id = parse_schedule_id(id_raw)?;
    Ok(ScheduleStateChangeInput {
        schedule_id,
        actor: cli_actor(),
        channel: Channel::Cli,
    })
}

fn render_transition(
    matches: &ArgMatches,
    output: inv_commands::ScheduleStateChangeOutput,
) -> Result<()> {
    let value = json!({
        "schedule": output.schedule,
        "emitted_events": event_topics(&output.emitted_events),
    });
    render_value(matches, value, &columns())
}

async fn run_show(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let id_raw = matches
        .get_one::<String>("id")
        .ok_or_else(|| anyhow!("schedule id required"))?;
    let schedule_id = parse_schedule_id(id_raw)?;
    let schedule = ScheduleRepo::new(&ctx.db)
        .get(&schedule_id)
        .await?
        .ok_or_else(|| anyhow!("schedule {schedule_id} not found"))?;
    render_value(matches, serde_json::to_value(schedule)?, &columns())?;
    Ok(())
}

async fn run_list(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let customer_id = match matches.get_one::<String>("customer") {
        None => None,
        Some(raw) => Some(parse_customer_id(raw)?),
    };
    let state = match matches.get_one::<String>("state") {
        None => None,
        Some(raw) => Some(parse_schedule_state(raw)?),
    };
    let limit = match matches.get_one::<String>("limit") {
        None => None,
        Some(raw) => Some(
            raw.parse::<i64>()
                .map_err(|e| anyhow!("invalid --limit `{raw}`: {e}"))?,
        ),
    };
    let offset = match matches.get_one::<String>("offset") {
        None => None,
        Some(raw) => Some(
            raw.parse::<i64>()
                .map_err(|e| anyhow!("invalid --offset `{raw}`: {e}"))?,
        ),
    };
    let filter = ScheduleFilter {
        customer_id,
        state,
        limit,
        offset,
    };
    let rows = ScheduleRepo::new(&ctx.db).list(&filter).await?;
    render_list(matches, serde_json::to_value(rows)?, &columns())?;
    Ok(())
}

fn parse_schedule_state(s: &str) -> Result<ScheduleState> {
    match s {
        "active" => Ok(ScheduleState::Active),
        "paused" => Ok(ScheduleState::Paused),
        "cancelled" => Ok(ScheduleState::Cancelled),
        other => Err(anyhow!(
            "invalid --state `{other}` (want active | paused | cancelled)"
        )),
    }
}

fn columns() -> Vec<ColumnSpec> {
    vec![
        ColumnSpec::new("id", "id", 26),
        ColumnSpec::new("state", "state", 10),
        ColumnSpec::new("cadence", "cadence", 14),
        ColumnSpec::new("currency", "currency", 8),
        ColumnSpec::new("next_run", "next_run", 12),
    ]
}

fn event_topics(events: &[inv_commands::EmittedEvent]) -> Value {
    json!(events
        .iter()
        .map(|e| e.topic.clone())
        .collect::<Vec<_>>())
}
