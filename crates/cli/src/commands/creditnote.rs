//! `inv creditnote …` subcommand tree.

use anyhow::{anyhow, Context, Result};
use clap::{Arg, ArgMatches, Command};
use hop_top_kit::output::ColumnSpec;
use serde_json::json;

use inv_commands::{
    create_credit_note, issue_credit_note, Actor, Channel, CoreCtx, CreateCreditNoteInput,
    IssueCreditNoteInput,
};
use inv_core::domain::creditnote::CreditNoteState;
use inv_store::repo::credit_note::{CreditNoteFilter, CreditNoteRepo};

use super::parse::{parse_credit_note_id, parse_decimal, parse_invoice_id};
use crate::render::{render_list, render_value};

fn cli_actor() -> Actor {
    Actor::Cli {
        name: std::env::var("USER").unwrap_or_else(|_| "cli".to_string()),
    }
}

/// Build the `inv creditnote` subcommand tree.
pub fn command() -> Command {
    Command::new("creditnote")
        .about("Credit-note lifecycle: draft, issue, show, list")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("draft")
                .about("Create a draft credit note against an invoice")
                .arg(
                    Arg::new("invoice")
                        .long("invoice")
                        .help("Invoice typeid")
                        .required(true),
                )
                .arg(
                    Arg::new("amount")
                        .long("amount")
                        .help("Credit amount (positive decimal)")
                        .required(true),
                )
                .arg(Arg::new("reason").long("reason").help("Free-form reason"))
                .arg(
                    Arg::new("idempotency-key")
                        .long("idempotency-key")
                        .help("Caller-supplied dedupe key"),
                ),
        )
        .subcommand(
            Command::new("issue")
                .about("Transition a credit note Draft → Issued")
                .arg(
                    Arg::new("id")
                        .help("Credit-note typeid")
                        .required(true)
                        .index(1),
                ),
        )
        .subcommand(
            Command::new("show").about("Show a single credit note").arg(
                Arg::new("id")
                    .help("Credit-note typeid")
                    .required(true)
                    .index(1),
            ),
        )
        .subcommand(
            Command::new("list")
                .about("List credit notes (optionally filtered)")
                .arg(Arg::new("invoice").long("invoice").help("Invoice typeid"))
                .arg(
                    Arg::new("state")
                        .long("state")
                        .help("Filter by lifecycle state (draft | issued)"),
                )
                .arg(Arg::new("limit").long("limit").help("Max rows"))
                .arg(Arg::new("offset").long("offset").help("Offset for paging")),
        )
}

/// Dispatch `inv creditnote …`.
pub async fn dispatch(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    match matches.subcommand() {
        Some(("draft", sub)) => run_draft(ctx, sub).await,
        Some(("issue", sub)) => run_issue(ctx, sub).await,
        Some(("show", sub)) => run_show(ctx, sub).await,
        Some(("list", sub)) => run_list(ctx, sub).await,
        _ => Err(anyhow!("creditnote: missing subcommand")),
    }
}

async fn run_draft(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let invoice_raw = matches
        .get_one::<String>("invoice")
        .ok_or_else(|| anyhow!("--invoice required"))?;
    let invoice_id = parse_invoice_id(invoice_raw)?;
    let amount_raw = matches
        .get_one::<String>("amount")
        .ok_or_else(|| anyhow!("--amount required"))?;
    let amount = parse_decimal("--amount", amount_raw)?;
    let reason = matches.get_one::<String>("reason").cloned();
    let idempotency_key = matches.get_one::<String>("idempotency-key").cloned();

    let input = CreateCreditNoteInput {
        invoice_id,
        amount,
        reason,
        refund_ref: None,
        idempotency_key,
        actor: cli_actor(),
        channel: Channel::Cli,
    };
    let output = create_credit_note(ctx, input)
        .await
        .context("create_credit_note failed")?;
    let value = json!({
        "credit_note": output.credit_note,
    });
    render_value(matches, value, &columns())?;
    Ok(())
}

async fn run_issue(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let id_raw = matches
        .get_one::<String>("id")
        .ok_or_else(|| anyhow!("credit-note id required"))?;
    let credit_note_id = parse_credit_note_id(id_raw)?;
    let input = IssueCreditNoteInput {
        credit_note_id,
        idempotency_key: None,
        actor: cli_actor(),
        channel: Channel::Cli,
    };
    let output = issue_credit_note(ctx, input)
        .await
        .context("issue_credit_note failed")?;
    let value = json!({
        "credit_note": output.credit_note,
    });
    render_value(matches, value, &columns())?;
    Ok(())
}

async fn run_show(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let id_raw = matches
        .get_one::<String>("id")
        .ok_or_else(|| anyhow!("credit-note id required"))?;
    let credit_note_id = parse_credit_note_id(id_raw)?;
    let cn = CreditNoteRepo::new(&ctx.db)
        .get(&credit_note_id)
        .await?
        .ok_or_else(|| anyhow!("credit note {credit_note_id} not found"))?;
    render_value(matches, serde_json::to_value(cn)?, &columns())?;
    Ok(())
}

async fn run_list(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let invoice_id = match matches.get_one::<String>("invoice") {
        None => None,
        Some(raw) => Some(parse_invoice_id(raw)?),
    };
    let state = match matches.get_one::<String>("state") {
        None => None,
        Some(raw) => Some(parse_credit_note_state(raw)?),
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
    let filter = CreditNoteFilter {
        invoice_id,
        state,
        limit,
        offset,
    };
    let rows = CreditNoteRepo::new(&ctx.db).list(&filter).await?;
    render_list(matches, serde_json::to_value(rows)?, &columns())?;
    Ok(())
}

fn parse_credit_note_state(s: &str) -> Result<CreditNoteState> {
    match s {
        "draft" => Ok(CreditNoteState::Draft),
        "issued" => Ok(CreditNoteState::Issued),
        other => Err(anyhow!("invalid --state `{other}` (want draft | issued)")),
    }
}

fn columns() -> Vec<ColumnSpec> {
    vec![
        ColumnSpec::new("id", "id", 26),
        ColumnSpec::new("number", "number", 14),
        ColumnSpec::new("state", "state", 8),
        ColumnSpec::new("amount", "amount", 12),
        ColumnSpec::new("currency", "currency", 8),
    ]
}
