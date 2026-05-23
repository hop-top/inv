//! Invoice tools.
//!
//! Maps to design §10's MCP-column entries for the invoice lifecycle:
//!
//! - `inv_invoice_draft`  → [`inv_commands::draft_invoice`]
//! - `inv_invoice_issue`  → [`inv_commands::issue_invoice`]
//! - `inv_invoice_send`   → [`inv_commands::send_invoice`]
//! - `inv_invoice_pay`    → [`inv_commands::mark_paid`]
//! - `inv_invoice_void`   → [`inv_commands::void_invoice`]
//! - `inv_invoice_show`   → `InvoiceRepo::get`
//! - `inv_invoice_list`   → `InvoiceRepo::list`
//! - `inv_customer_add`   → `CustomerRepo::save`
//! - `inv_customer_show`  → `CustomerRepo::get`
//! - `inv_customer_list`  → `CustomerRepo::list`
//! - `inv_tax_rates_show` → dump `CoreCtx::tax_table`

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use inv_commands::{
    draft_invoice, issue_invoice, mark_paid, void_invoice, DraftInvoiceInput, DraftLineInput,
    IssueInvoiceInput, MarkPaidInput, VoidInvoiceInput,
};
use inv_core::domain::address::Address;
use inv_core::domain::customer::Customer;
use inv_core::domain::ids::{CustomerId, InvoiceId};
use inv_core::domain::invoice::{InvoiceState, TaxCategory};
use inv_core::domain::jurisdiction::Jurisdiction;
use inv_core::domain::money::Currency;
use inv_store::repo::customer::CustomerRepo;
use inv_store::repo::invoice::{InvoiceFilter, InvoiceLineRepo, InvoiceRepo};
use std::collections::BTreeMap;

use inv_commands::CoreCtx;

use crate::error::McpError;
use crate::tools::common::{mcp_actor, mcp_channel};

// =============================================================================
// inv_invoice_draft
// =============================================================================

/// Input for `inv_invoice_draft`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DraftLineWire {
    /// Free-form description of the line item.
    pub description: String,
    /// Quantity (must be > 0). Encoded as a decimal-string.
    #[schemars(with = "String")]
    pub quantity: Decimal,
    /// Per-unit price in invoice currency. Encoded as a decimal-string.
    #[schemars(with = "String")]
    pub unit_price: Decimal,
    /// Tax category override (default `standard`).
    #[serde(default)]
    pub tax_category: Option<TaxCategory>,
}

/// Input for `inv_invoice_draft`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvoiceDraftInput {
    /// Bill-to customer id (MTI; must already exist).
    pub customer_id: String,
    /// Seller jurisdiction (drives tax resolution at issue). One of
    /// `CA-QC`, `US-DE`, `DZ-16` (see
    /// `inv_core::domain::jurisdiction::Jurisdiction`).
    pub seller_jurisdiction: Jurisdiction,
    /// ISO 4217 currency code (USD, CAD, or DZD at v1).
    pub currency: String,
    /// At least one line.
    pub lines: Vec<DraftLineWire>,
    /// Optional caller-supplied dedupe key.
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// Optional payment due date (ISO 8601).
    #[serde(default)]
    pub due_at: Option<DateTime<Utc>>,
    /// Optional per-invoice template override (filesystem path).
    #[serde(default)]
    pub template_path: Option<String>,
}

/// Wrap the command output as a `serde_json::Value` containing the
/// fully serialized invoice + lines + replay flag. T-0043 dropped the
/// `emitted_events` field — events now publish on the bus directly via
/// `ctx.publisher`; agents that need them should subscribe to the bus.
#[derive(Debug, Serialize)]
struct DraftOutputWire<'a> {
    invoice: &'a inv_core::domain::invoice::Invoice,
    lines: &'a [inv_core::domain::invoice::InvoiceLine],
    idempotency_replay: bool,
}

/// Run `draft_invoice`.
pub async fn draft(ctx: &CoreCtx, input: InvoiceDraftInput) -> Result<serde_json::Value, McpError> {
    let customer_id: CustomerId =
        input
            .customer_id
            .parse()
            .map_err(|e: inv_core::domain::ids::IdError| {
                McpError::Decode(format!("customer_id: {e}"))
            })?;
    let currency =
        Currency::new(&input.currency).map_err(|e| McpError::Decode(format!("currency: {e}")))?;
    let lines = input
        .lines
        .into_iter()
        .map(|l| DraftLineInput {
            description: l.description,
            quantity: l.quantity,
            unit_price: l.unit_price,
            tax_category: l.tax_category.unwrap_or_default(),
        })
        .collect();

    let req = DraftInvoiceInput {
        customer_id,
        seller_jurisdiction: input.seller_jurisdiction,
        currency,
        lines,
        idempotency_key: input.idempotency_key,
        actor: mcp_actor(),
        channel: mcp_channel(),
        due_at: input.due_at,
        template_path: input.template_path,
        schedule_id: None,
    };
    let out = draft_invoice(ctx, req).await?;
    let wire = DraftOutputWire {
        invoice: &out.invoice,
        lines: &out.lines,
        idempotency_replay: out.idempotency_replay,
    };
    crate::tools::common::to_value(&wire)
}

// =============================================================================
// inv_invoice_issue
// =============================================================================

/// Input for `inv_invoice_issue`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvoiceIssueInput {
    /// Invoice id.
    pub invoice_id: String,
    /// Optional caller-supplied dedupe key.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Serialize)]
struct IssueOutputWire<'a> {
    invoice: &'a inv_core::domain::invoice::Invoice,
    lines: &'a [inv_core::domain::invoice::InvoiceLine],
    /// HTML body. Always present.
    html: &'a str,
    /// Length of the rendered PDF bytes (the stub engine emits HTML
    /// verbatim at v1; we expose length only to keep tool outputs JSON
    /// without base64-blasting megabytes).
    pdf_len: usize,
}

/// Run `issue_invoice`.
pub async fn issue(ctx: &CoreCtx, input: InvoiceIssueInput) -> Result<serde_json::Value, McpError> {
    let invoice_id: InvoiceId =
        input
            .invoice_id
            .parse()
            .map_err(|e: inv_core::domain::ids::IdError| {
                McpError::Decode(format!("invoice_id: {e}"))
            })?;
    let req = IssueInvoiceInput {
        invoice_id,
        idempotency_key: input.idempotency_key,
        actor: mcp_actor(),
        channel: mcp_channel(),
    };
    let out = issue_invoice(ctx, req).await?;
    let wire = IssueOutputWire {
        invoice: &out.invoice,
        lines: &out.lines,
        html: &out.html,
        pdf_len: out.pdf.len(),
    };
    crate::tools::common::to_value(&wire)
}

// =============================================================================
// inv_invoice_send
// =============================================================================

/// Input for `inv_invoice_send`.
///
/// Supported destination schemes at v1: `file://<path>`, `stdout`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvoiceSendInput {
    /// Invoice id.
    pub invoice_id: String,
    /// Destination URI: `file://<path>` or `stdout`.
    pub destination_uri: String,
    /// Optional caller-supplied dedupe key.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Serialize)]
struct SendOutputWire<'a> {
    invoice: &'a inv_core::domain::invoice::Invoice,
    delivered_to: &'a str,
    bytes_written: usize,
}

/// Run `send_invoice`. The `stdout` scheme uses an in-memory `Vec<u8>`
/// sink (the real process stdout is reserved for the MCP transport at
/// v1; sending to stdout in a stdio-transported MCP server would
/// corrupt the JSON-RPC stream).
pub async fn send(ctx: &CoreCtx, input: InvoiceSendInput) -> Result<serde_json::Value, McpError> {
    use inv_commands::SendInvoiceInput;
    let invoice_id: InvoiceId =
        input
            .invoice_id
            .parse()
            .map_err(|e: inv_core::domain::ids::IdError| {
                McpError::Decode(format!("invoice_id: {e}"))
            })?;
    let mut sink: Vec<u8> = Vec::new();
    let cmd_input = SendInvoiceInput {
        invoice_id,
        destination_uri: input.destination_uri,
        idempotency_key: input.idempotency_key,
        actor: mcp_actor(),
        channel: mcp_channel(),
        sink: Some(&mut sink),
    };
    let out = inv_commands::send_invoice(ctx, cmd_input).await?;
    let wire = SendOutputWire {
        invoice: &out.invoice,
        delivered_to: &out.delivered_to,
        bytes_written: sink.len(),
    };
    crate::tools::common::to_value(&wire)
}

// =============================================================================
// inv_invoice_pay
// =============================================================================

/// Input for `inv_invoice_pay`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvoicePayInput {
    /// Invoice id receiving the payment.
    pub invoice_id: String,
    /// Payment amount (> 0). Decimal-string.
    #[schemars(with = "String")]
    pub amount: Decimal,
    /// When the payment was received. Defaults to "now" if omitted.
    #[serde(default)]
    pub received_at: Option<DateTime<Utc>>,
    /// Optional caller-supplied dedupe key.
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// Inbound bus event id (only set when triggered by a bus event).
    #[serde(default)]
    pub bus_event_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct PayOutputWire<'a> {
    invoice: &'a inv_core::domain::invoice::Invoice,
    fully_paid: bool,
}

/// Run `mark_paid`.
pub async fn pay(ctx: &CoreCtx, input: InvoicePayInput) -> Result<serde_json::Value, McpError> {
    let invoice_id: InvoiceId =
        input
            .invoice_id
            .parse()
            .map_err(|e: inv_core::domain::ids::IdError| {
                McpError::Decode(format!("invoice_id: {e}"))
            })?;
    let req = MarkPaidInput {
        invoice_id,
        amount: input.amount,
        received_at: input.received_at,
        idempotency_key: input.idempotency_key,
        bus_event_id: input.bus_event_id,
        actor: mcp_actor(),
        channel: mcp_channel(),
    };
    let out = mark_paid(ctx, req).await?;
    let wire = PayOutputWire {
        invoice: &out.invoice,
        fully_paid: out.fully_paid,
    };
    crate::tools::common::to_value(&wire)
}

// =============================================================================
// inv_invoice_void
// =============================================================================

/// Input for `inv_invoice_void`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvoiceVoidInput {
    /// Invoice id.
    pub invoice_id: String,
    /// Optional free-form void reason.
    #[serde(default)]
    pub reason: Option<String>,
    /// Optional caller-supplied dedupe key.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// Run `void_invoice`.
pub async fn void(ctx: &CoreCtx, input: InvoiceVoidInput) -> Result<serde_json::Value, McpError> {
    let invoice_id: InvoiceId =
        input
            .invoice_id
            .parse()
            .map_err(|e: inv_core::domain::ids::IdError| {
                McpError::Decode(format!("invoice_id: {e}"))
            })?;
    let req = VoidInvoiceInput {
        invoice_id,
        reason: input.reason,
        idempotency_key: input.idempotency_key,
        actor: mcp_actor(),
        channel: mcp_channel(),
    };
    let out = void_invoice(ctx, req).await?;
    crate::tools::common::to_value(&out.invoice)
}

// =============================================================================
// inv_invoice_show / inv_invoice_list
// =============================================================================

/// Input for `inv_invoice_show`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvoiceShowInput {
    /// Invoice id.
    pub invoice_id: String,
}

/// Show an invoice.
///
/// Output matches `inv://invoice/<id>` resource read byte-for-byte
/// (T-0041): bare `Invoice` fields flattened at the top level, plus a
/// `lines` array joined from `invoice_lines` (ordered by `position`).
/// Serialised via [`crate::resources::InvoiceResourceBody`] — the same
/// wrapper the resource handler uses.
pub async fn show(ctx: &CoreCtx, input: InvoiceShowInput) -> Result<serde_json::Value, McpError> {
    let id: InvoiceId = input
        .invoice_id
        .parse()
        .map_err(|e: inv_core::domain::ids::IdError| {
            McpError::Decode(format!("invoice_id: {e}"))
        })?;
    let inv = InvoiceRepo::new(&ctx.db)
        .get(&id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("invoice {id}")))?;
    let lines = InvoiceLineRepo::new(&ctx.db).list_for_invoice(&id).await?;
    let body = crate::resources::InvoiceResourceBody {
        invoice: &inv,
        lines: &lines,
    };
    crate::tools::common::to_value(&body)
}

/// Input for `inv_invoice_list`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct InvoiceListInput {
    /// Restrict to a customer id (MTI).
    #[serde(default)]
    pub customer_id: Option<String>,
    /// Restrict to a state (`draft`, `issued`, `sent`, `viewed`,
    /// `partially_paid`, `paid`, `voided`).
    #[serde(default)]
    pub state: Option<InvoiceState>,
    /// Max rows.
    #[serde(default)]
    pub limit: Option<i64>,
    /// Offset (paging).
    #[serde(default)]
    pub offset: Option<i64>,
}

/// List invoices.
pub async fn list(ctx: &CoreCtx, input: InvoiceListInput) -> Result<serde_json::Value, McpError> {
    let customer_id = input
        .customer_id
        .as_deref()
        .map(|s| {
            s.parse::<CustomerId>()
                .map_err(|e: inv_core::domain::ids::IdError| {
                    McpError::Decode(format!("customer_id: {e}"))
                })
        })
        .transpose()?;
    let filter = InvoiceFilter {
        customer_id,
        state: input.state,
        limit: input.limit,
        offset: input.offset,
    };
    let invs = InvoiceRepo::new(&ctx.db).list(&filter).await?;
    crate::tools::common::to_value(&invs)
}

// =============================================================================
// inv_customer_add / inv_customer_show / inv_customer_list
// =============================================================================

/// Input for `inv_customer_add`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CustomerAddInput {
    /// Optional explicit id (MTI). When omitted, a fresh id is minted.
    #[serde(default)]
    pub customer_id: Option<String>,
    /// Display name.
    pub display_name: String,
    /// Optional email.
    #[serde(default)]
    pub email: Option<String>,
    /// Postal address.
    pub address: Address,
}

/// Upsert a customer.
pub async fn customer_add(
    ctx: &CoreCtx,
    input: CustomerAddInput,
) -> Result<serde_json::Value, McpError> {
    let id = match input.customer_id {
        Some(s) => s
            .parse::<CustomerId>()
            .map_err(|e: inv_core::domain::ids::IdError| {
                McpError::Decode(format!("customer_id: {e}"))
            })?,
        None => CustomerId::new(),
    };
    let now = ctx.clock.now();
    let customer = Customer {
        id,
        display_name: input.display_name,
        email: input.email,
        address: input.address,
        metadata: BTreeMap::new(),
        created_at: now,
        updated_at: now,
    };
    CustomerRepo::new(&ctx.db).save(&customer).await?;
    crate::tools::common::to_value(&customer)
}

/// Input for `inv_customer_show`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CustomerShowInput {
    /// Customer id (MTI).
    pub customer_id: String,
}

/// Show a customer.
pub async fn customer_show(
    ctx: &CoreCtx,
    input: CustomerShowInput,
) -> Result<serde_json::Value, McpError> {
    let id: CustomerId =
        input
            .customer_id
            .parse()
            .map_err(|e: inv_core::domain::ids::IdError| {
                McpError::Decode(format!("customer_id: {e}"))
            })?;
    let c = CustomerRepo::new(&ctx.db)
        .get(&id)
        .await?
        .ok_or_else(|| McpError::NotFound(format!("customer {id}")))?;
    crate::tools::common::to_value(&c)
}

/// Input for `inv_customer_list`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CustomerListInput {
    /// Max rows (default 50).
    #[serde(default = "default_customer_limit")]
    pub limit: i64,
    /// Offset (default 0).
    #[serde(default)]
    pub offset: i64,
}

fn default_customer_limit() -> i64 {
    50
}

impl Default for CustomerListInput {
    fn default() -> Self {
        Self {
            limit: default_customer_limit(),
            offset: 0,
        }
    }
}

/// List customers.
pub async fn customer_list(
    ctx: &CoreCtx,
    input: CustomerListInput,
) -> Result<serde_json::Value, McpError> {
    let list = CustomerRepo::new(&ctx.db)
        .list(input.limit, input.offset)
        .await?;
    crate::tools::common::to_value(&list)
}

// =============================================================================
// inv_tax_rates_show
// =============================================================================

/// Dump the loaded tax-table + nexus config.
pub async fn tax_rates_show(ctx: &CoreCtx) -> Result<serde_json::Value, McpError> {
    crate::tools::common::to_value(&serde_json::json!({
        "rates": ctx.tax_table.rates,
        "nexus_states": ctx.nexus.states,
        "rate_count": ctx.tax_table.len(),
    }))
}
