//! `inv invoice …` subcommand tree.

use anyhow::{anyhow, Context, Result};
use clap::{value_parser, Arg, ArgAction, ArgMatches, Command};
use hop_top_kit::output::ColumnSpec;
use serde_json::{json, Value};

use inv_commands::send::StdoutSink;
use inv_commands::{
    draft_invoice, issue_invoice, mark_paid, send_invoice, void_invoice, Actor,
    Channel, CoreCtx, DraftInvoiceInput, DraftLineInput, IssueInvoiceInput,
    MarkPaidInput, SendInvoiceInput, VoidInvoiceInput,
};
use inv_core::domain::invoice::{InvoiceState, TaxCategory};
use inv_core::domain::jurisdiction::Jurisdiction;
use inv_core::domain::money::Currency;
use inv_store::repo::invoice::{InvoiceFilter, InvoiceLineRepo, InvoiceRepo};

use super::parse::{parse_customer_id, parse_decimal, parse_invoice_id, parse_line, LineSpec};
use crate::render::{render_list, render_value};

/// Default actor for CLI commands — `$USER` if set, otherwise `"cli"`.
fn cli_actor() -> Actor {
    let name = std::env::var("USER").unwrap_or_else(|_| "cli".to_string());
    Actor::Cli { name }
}

/// Build the `inv invoice` clap subcommand tree.
pub fn command() -> Command {
    Command::new("invoice")
        .about("Invoice lifecycle: draft, issue, send, pay, void, show, list")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(draft_cmd())
        .subcommand(issue_cmd())
        .subcommand(send_cmd())
        .subcommand(pay_cmd())
        .subcommand(void_cmd())
        .subcommand(show_cmd())
        .subcommand(list_cmd())
}

fn draft_cmd() -> Command {
    Command::new("draft")
        .about("Create a new draft invoice")
        .arg(
            Arg::new("customer")
                .long("customer")
                .help("Customer typeid (e.g. customer_01...)")
                .required(true),
        )
        .arg(
            Arg::new("currency")
                .long("currency")
                .help("Invoice currency (USD | CAD | DZD)")
                .required(true),
        )
        .arg(
            Arg::new("seller-jurisdiction")
                .long("seller-jurisdiction")
                .help("Seller jurisdiction (CA-QC | US-DE | DZ-16)")
                .default_value("CA-QC"),
        )
        .arg(
            Arg::new("line")
                .long("line")
                .help("Line as `<desc>:<qty>:<price>` (repeatable; at least one required)")
                .action(ArgAction::Append)
                .required(true),
        )
        .arg(
            Arg::new("due")
                .long("due")
                .help("Due date as RFC 3339 timestamp"),
        )
        .arg(
            Arg::new("idempotency-key")
                .long("idempotency-key")
                .help("Caller-supplied dedupe key"),
        )
        .arg(
            Arg::new("template-path")
                .long("template-path")
                .help("Per-invoice template override (filesystem path)"),
        )
}

fn issue_cmd() -> Command {
    Command::new("issue")
        .about("Transition Draft → Issued and freeze the invoice")
        .arg(
            Arg::new("id")
                .help("Invoice typeid")
                .required(true)
                .index(1),
        )
}

fn send_cmd() -> Command {
    Command::new("send")
        .about("Send an issued invoice to a destination URI (file:// | stdout)")
        .arg(
            Arg::new("id")
                .help("Invoice typeid")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::new("to")
                .long("to")
                .help("Destination URI (file://<path> | stdout | bus://... | webhook://... | link://...)")
                .required(true),
        )
}

fn pay_cmd() -> Command {
    Command::new("pay")
        .about("Record a payment against an invoice")
        .arg(
            Arg::new("id")
                .help("Invoice typeid")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::new("amount")
                .long("amount")
                .help("Payment amount (decimal, positive)")
                .required(true),
        )
        .arg(
            Arg::new("idempotency-key")
                .long("idempotency-key")
                .help("Caller-supplied dedupe key"),
        )
        .arg(
            Arg::new("bus-event-id")
                .long("bus-event-id")
                .help("Inbound bus event id (rarely supplied from the CLI)"),
        )
}

fn void_cmd() -> Command {
    Command::new("void")
        .about("Void an issued/sent/viewed invoice (pre-payment only)")
        .arg(
            Arg::new("id")
                .help("Invoice typeid")
                .required(true)
                .index(1),
        )
        .arg(
            Arg::new("reason")
                .long("reason")
                .help("Free-form void reason"),
        )
}

fn show_cmd() -> Command {
    Command::new("show")
        .about("Show a single invoice (with lines)")
        .arg(
            Arg::new("id")
                .help("Invoice typeid")
                .required(true)
                .index(1),
        )
}

fn list_cmd() -> Command {
    Command::new("list")
        .about("List invoices")
        .arg(
            Arg::new("customer")
                .long("customer")
                .help("Filter by customer typeid"),
        )
        .arg(
            Arg::new("state")
                .long("state")
                .help("Filter by lifecycle state (draft|issued|sent|viewed|partially_paid|paid|voided)"),
        )
        .arg(
            Arg::new("limit")
                .long("limit")
                .value_parser(value_parser!(i64))
                .help("Max rows to return"),
        )
        .arg(
            Arg::new("offset")
                .long("offset")
                .value_parser(value_parser!(i64))
                .help("Pagination offset"),
        )
}

// =============================================================================
// Dispatch
// =============================================================================

/// Top-level dispatch for `inv invoice ...`.
pub async fn dispatch(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    match matches.subcommand() {
        Some(("draft", sub)) => run_draft(ctx, sub).await,
        Some(("issue", sub)) => run_issue(ctx, sub).await,
        Some(("send", sub)) => run_send(ctx, sub).await,
        Some(("pay", sub)) => run_pay(ctx, sub).await,
        Some(("void", sub)) => run_void(ctx, sub).await,
        Some(("show", sub)) => run_show(ctx, sub).await,
        Some(("list", sub)) => run_list(ctx, sub).await,
        _ => Err(anyhow!("invoice: missing subcommand")),
    }
}

/// Build a [`DraftInvoiceInput`] from `inv invoice draft …` matches.
///
/// Exposed for unit-testing the arg → input mapping.
pub fn build_draft_input(matches: &ArgMatches, actor: Actor) -> Result<DraftInvoiceInput> {
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

    let seller_raw = matches
        .get_one::<String>("seller-jurisdiction")
        .cloned()
        .unwrap_or_else(|| "CA-QC".to_string());
    let seller_jurisdiction: Jurisdiction = seller_raw
        .parse()
        .map_err(|e| anyhow!("invalid --seller-jurisdiction `{seller_raw}`: {e}"))?;

    let line_specs: Vec<LineSpec> = matches
        .get_many::<String>("line")
        .map(|it| it.map(|s| parse_line(s)).collect::<Result<Vec<_>>>())
        .transpose()?
        .unwrap_or_default();
    if line_specs.is_empty() {
        return Err(anyhow!("at least one --line is required"));
    }
    let lines: Vec<DraftLineInput> = line_specs
        .into_iter()
        .map(|l| DraftLineInput {
            description: l.description,
            quantity: l.quantity,
            unit_price: l.unit_price,
            tax_category: TaxCategory::Standard,
        })
        .collect();

    let due_at = match matches.get_one::<String>("due") {
        None => None,
        Some(raw) => Some(super::parse::parse_datetime("--due", raw)?),
    };

    let idempotency_key = matches.get_one::<String>("idempotency-key").cloned();
    let template_path = matches.get_one::<String>("template-path").cloned();

    Ok(DraftInvoiceInput {
        customer_id,
        seller_jurisdiction,
        currency,
        lines,
        idempotency_key,
        actor,
        channel: Channel::Cli,
        due_at,
        template_path,
    })
}

async fn run_draft(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let input = build_draft_input(matches, cli_actor())?;
    let output = draft_invoice(ctx, input).await.context("draft_invoice failed")?;
    let value = json!({
        "invoice": output.invoice,
        "lines": output.lines,
        "idempotency_replay": output.idempotency_replay,
        "emitted_events": event_topics(&output.emitted_events),
    });
    render_value(matches, value, &invoice_columns())?;
    Ok(())
}

/// Build the [`IssueInvoiceInput`] from `inv invoice issue <id>`.
pub fn build_issue_input(matches: &ArgMatches, actor: Actor) -> Result<IssueInvoiceInput> {
    let id_raw = matches
        .get_one::<String>("id")
        .ok_or_else(|| anyhow!("invoice id required"))?;
    let invoice_id = parse_invoice_id(id_raw)?;
    Ok(IssueInvoiceInput {
        invoice_id,
        idempotency_key: None,
        actor,
        channel: Channel::Cli,
    })
}

async fn run_issue(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let input = build_issue_input(matches, cli_actor())?;
    let output = issue_invoice(ctx, input).await.context("issue_invoice failed")?;
    let value = json!({
        "invoice": output.invoice,
        "lines": output.lines,
        "html_len": output.html.len(),
        "pdf_len": output.pdf.len(),
        "emitted_events": event_topics(&output.emitted_events),
    });
    render_value(matches, value, &invoice_columns())?;
    Ok(())
}

async fn run_send(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let id_raw = matches
        .get_one::<String>("id")
        .ok_or_else(|| anyhow!("invoice id required"))?;
    let invoice_id = parse_invoice_id(id_raw)?;
    let to = matches
        .get_one::<String>("to")
        .ok_or_else(|| anyhow!("--to required"))?
        .clone();

    let mut stdout_sink = StdoutSink;
    let input = SendInvoiceInput {
        invoice_id,
        destination_uri: to,
        idempotency_key: None,
        actor: cli_actor(),
        channel: Channel::Cli,
        sink: Some(&mut stdout_sink as &mut dyn inv_commands::SendSink),
    };
    let output = send_invoice(ctx, input).await.context("send_invoice failed")?;
    let value = json!({
        "invoice": output.invoice,
        "delivered_to": output.delivered_to,
        "html_len": output.html.len(),
        "pdf_len": output.pdf.len(),
        "emitted_events": event_topics(&output.emitted_events),
    });
    render_value(matches, value, &invoice_columns())?;
    Ok(())
}

/// Build the [`MarkPaidInput`] from `inv invoice pay <id> --amount`.
pub fn build_pay_input(matches: &ArgMatches, actor: Actor) -> Result<MarkPaidInput> {
    let id_raw = matches
        .get_one::<String>("id")
        .ok_or_else(|| anyhow!("invoice id required"))?;
    let invoice_id = parse_invoice_id(id_raw)?;
    let amount_raw = matches
        .get_one::<String>("amount")
        .ok_or_else(|| anyhow!("--amount required"))?;
    let amount = parse_decimal("--amount", amount_raw)?;
    Ok(MarkPaidInput {
        invoice_id,
        amount,
        received_at: None,
        idempotency_key: matches.get_one::<String>("idempotency-key").cloned(),
        bus_event_id: matches.get_one::<String>("bus-event-id").cloned(),
        actor,
        channel: Channel::Cli,
    })
}

async fn run_pay(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let input = build_pay_input(matches, cli_actor())?;
    let output = mark_paid(ctx, input).await.context("mark_paid failed")?;
    let value = json!({
        "invoice": output.invoice,
        "fully_paid": output.fully_paid,
        "emitted_events": event_topics(&output.emitted_events),
    });
    render_value(matches, value, &invoice_columns())?;
    Ok(())
}

async fn run_void(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let id_raw = matches
        .get_one::<String>("id")
        .ok_or_else(|| anyhow!("invoice id required"))?;
    let invoice_id = parse_invoice_id(id_raw)?;
    let reason = matches.get_one::<String>("reason").cloned();
    let input = VoidInvoiceInput {
        invoice_id,
        reason,
        idempotency_key: None,
        actor: cli_actor(),
        channel: Channel::Cli,
    };
    let output = void_invoice(ctx, input).await.context("void_invoice failed")?;
    let value = json!({
        "invoice": output.invoice,
        "emitted_events": event_topics(&output.emitted_events),
    });
    render_value(matches, value, &invoice_columns())?;
    Ok(())
}

async fn run_show(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let id_raw = matches
        .get_one::<String>("id")
        .ok_or_else(|| anyhow!("invoice id required"))?;
    let invoice_id = parse_invoice_id(id_raw)?;
    let inv_repo = InvoiceRepo::new(&ctx.db);
    let line_repo = InvoiceLineRepo::new(&ctx.db);
    let invoice = inv_repo
        .get(&invoice_id)
        .await?
        .ok_or_else(|| anyhow!("invoice {invoice_id} not found"))?;
    let lines = line_repo.list_for_invoice(&invoice_id).await?;
    let value = json!({
        "invoice": invoice,
        "lines": lines,
    });
    render_value(matches, value, &invoice_columns())?;
    Ok(())
}

async fn run_list(ctx: &CoreCtx, matches: &ArgMatches) -> Result<()> {
    let customer_id = match matches.get_one::<String>("customer") {
        None => None,
        Some(raw) => Some(parse_customer_id(raw)?),
    };
    let state = match matches.get_one::<String>("state") {
        None => None,
        Some(raw) => Some(parse_invoice_state(raw)?),
    };
    let filter = InvoiceFilter {
        customer_id,
        state,
        limit: matches.get_one::<i64>("limit").copied(),
        offset: matches.get_one::<i64>("offset").copied(),
    };
    let rows = InvoiceRepo::new(&ctx.db).list(&filter).await?;
    let value = serde_json::to_value(rows)?;
    render_list(matches, value, &invoice_columns())?;
    Ok(())
}

fn parse_invoice_state(raw: &str) -> Result<InvoiceState> {
    Ok(match raw {
        "draft" => InvoiceState::Draft,
        "issued" => InvoiceState::Issued,
        "sent" => InvoiceState::Sent,
        "viewed" => InvoiceState::Viewed,
        "partially_paid" => InvoiceState::PartiallyPaid,
        "paid" => InvoiceState::Paid,
        "voided" => InvoiceState::Voided,
        other => return Err(anyhow!("unknown invoice state `{other}`")),
    })
}

fn invoice_columns() -> Vec<ColumnSpec> {
    vec![
        ColumnSpec::new("id", "id", 26),
        ColumnSpec::new("number", "number", 14),
        ColumnSpec::new("state", "state", 14),
        ColumnSpec::new("total", "total", 12),
        ColumnSpec::new("currency", "currency", 8),
    ]
}

fn event_topics(events: &[inv_commands::EmittedEvent]) -> Value {
    json!(events
        .iter()
        .map(|e| e.topic.clone())
        .collect::<Vec<_>>())
}
