//! [`InvMcpServer`] — the rmcp `ServerHandler` impl that owns
//! [`CoreCtx`] and routes every MCP request to the appropriate tool /
//! resource helper.
//!
//! Tools are registered via the `#[tool]` / `#[tool_router]` /
//! `#[tool_handler]` proc-macros shipped by rmcp 1.7. Resources are
//! served via the explicit `list_resources` / `list_resource_templates`
//! / `read_resource` methods on `ServerHandler`.

use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResult, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_handler, tool_router, ErrorData as RmcpError, RoleServer, ServerHandler};

use inv_commands::CoreCtx;

use crate::resources;
use crate::tools::credit_notes::{CreditNoteDraftInput, CreditNoteIssueInput};
use crate::tools::invoices::{
    CustomerAddInput, CustomerListInput, CustomerShowInput, InvoiceDraftInput, InvoiceIssueInput,
    InvoiceListInput, InvoicePayInput, InvoiceSendInput, InvoiceShowInput, InvoiceVoidInput,
};
use crate::tools::reminders::{ReminderCancelWire, ReminderScheduleWire};
use crate::tools::schedules::{ScheduleCreateWire, ScheduleStateChangeWire};
use crate::tools::{credit_notes, invoices, reminders, schedules, tickers};

/// MCP server adapter for the `inv` workspace.
///
/// Owns an `Arc<CoreCtx>` so the rmcp tool-router (which calls
/// `&self`-receiver tool methods) can share the context across
/// concurrent tool invocations without locking.
#[derive(Clone)]
pub struct InvMcpServer {
    ctx: Arc<CoreCtx>,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for InvMcpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvMcpServer")
            .field("ctx", &"<CoreCtx>")
            .finish()
    }
}

#[tool_router(router = tool_router)]
impl InvMcpServer {
    /// Construct from a shared `CoreCtx`.
    pub fn new(ctx: Arc<CoreCtx>) -> Self {
        Self {
            ctx,
            tool_router: Self::tool_router(),
        }
    }

    /// Shared `CoreCtx` accessor (useful in tests and resource handler).
    pub fn ctx(&self) -> &Arc<CoreCtx> {
        &self.ctx
    }

    // -----------------------------------------------------------------
    // Invoice tools
    // -----------------------------------------------------------------

    /// Draft a new invoice (creates lines + history row in one tx).
    #[tool(
        name = "inv_invoice_draft",
        description = "Create a new draft invoice. Lines are pre-tax; tax is frozen at issue time."
    )]
    pub async fn inv_invoice_draft(
        &self,
        Parameters(input): Parameters<InvoiceDraftInput>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(invoices::draft(&self.ctx, input).await)
    }

    /// Transition a draft to issued.
    #[tool(
        name = "inv_invoice_issue",
        description = "Transition a draft invoice to Issued: assigns a number, snapshots tax, renders HTML+PDF."
    )]
    pub async fn inv_invoice_issue(
        &self,
        Parameters(input): Parameters<InvoiceIssueInput>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(invoices::issue(&self.ctx, input).await)
    }

    /// Deliver an issued invoice to a destination URI.
    #[tool(
        name = "inv_invoice_send",
        description = "Deliver an issued invoice. v1 supports `file://<path>` and `stdout` schemes (the stdout sink is captured in-memory under the MCP transport)."
    )]
    pub async fn inv_invoice_send(
        &self,
        Parameters(input): Parameters<InvoiceSendInput>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(invoices::send(&self.ctx, input).await)
    }

    /// Record a payment against an invoice.
    #[tool(
        name = "inv_invoice_pay",
        description = "Record a payment. Amount must be > 0; transitions the invoice to PartiallyPaid or Paid based on cumulative amount."
    )]
    pub async fn inv_invoice_pay(
        &self,
        Parameters(input): Parameters<InvoicePayInput>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(invoices::pay(&self.ctx, input).await)
    }

    /// Void an issued/sent/viewed invoice (pre-payment only).
    #[tool(
        name = "inv_invoice_void",
        description = "Void an issued/sent/viewed invoice (pre-payment only). Terminal."
    )]
    pub async fn inv_invoice_void(
        &self,
        Parameters(input): Parameters<InvoiceVoidInput>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(invoices::void(&self.ctx, input).await)
    }

    /// Show an invoice by id.
    #[tool(
        name = "inv_invoice_show",
        description = "Show a single invoice (does not include lines; use the `inv://invoice/<id>` resource for full detail)."
    )]
    pub async fn inv_invoice_show(
        &self,
        Parameters(input): Parameters<InvoiceShowInput>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(invoices::show(&self.ctx, input).await)
    }

    /// List invoices with an optional state/customer filter.
    #[tool(
        name = "inv_invoice_list",
        description = "List invoices. Optional filters: customer_id (MTI), state, limit, offset."
    )]
    pub async fn inv_invoice_list(
        &self,
        Parameters(input): Parameters<InvoiceListInput>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(invoices::list(&self.ctx, input).await)
    }

    // -----------------------------------------------------------------
    // Customer + tax tools
    // -----------------------------------------------------------------

    /// Upsert a customer.
    #[tool(
        name = "inv_customer_add",
        description = "Upsert a customer (creates or updates by id; if no id is supplied a fresh one is minted)."
    )]
    pub async fn inv_customer_add(
        &self,
        Parameters(input): Parameters<CustomerAddInput>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(invoices::customer_add(&self.ctx, input).await)
    }

    /// Show a customer.
    #[tool(
        name = "inv_customer_show",
        description = "Show a single customer by id."
    )]
    pub async fn inv_customer_show(
        &self,
        Parameters(input): Parameters<CustomerShowInput>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(invoices::customer_show(&self.ctx, input).await)
    }

    /// List customers.
    #[tool(
        name = "inv_customer_list",
        description = "List customers (paged; default limit 50)."
    )]
    pub async fn inv_customer_list(
        &self,
        Parameters(input): Parameters<CustomerListInput>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(invoices::customer_list(&self.ctx, input).await)
    }

    /// Dump the loaded tax table + nexus config.
    #[tool(
        name = "inv_tax_rates_show",
        description = "Return the loaded tax-rate table and US nexus configuration."
    )]
    pub async fn inv_tax_rates_show(&self) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(invoices::tax_rates_show(&self.ctx).await)
    }

    // -----------------------------------------------------------------
    // Credit-note tools
    // -----------------------------------------------------------------

    /// Draft a credit note against an invoice.
    #[tool(
        name = "inv_creditnote_draft",
        description = "Draft a credit note against an invoice. Amount must be > 0."
    )]
    pub async fn inv_creditnote_draft(
        &self,
        Parameters(input): Parameters<CreditNoteDraftInput>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(credit_notes::draft(&self.ctx, input).await)
    }

    /// Issue a draft credit note.
    #[tool(
        name = "inv_creditnote_issue",
        description = "Transition a draft credit note to Issued (assigns a CN-YYYY-NNNN number)."
    )]
    pub async fn inv_creditnote_issue(
        &self,
        Parameters(input): Parameters<CreditNoteIssueInput>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(credit_notes::issue(&self.ctx, input).await)
    }

    // -----------------------------------------------------------------
    // Schedule tools
    // -----------------------------------------------------------------

    /// Create a recurring schedule.
    #[tool(
        name = "inv_schedule_create",
        description = "Create an active recurring schedule. Cadence: `monthly@<dom>` | `quarterly@<dom>` | `yearly@MM-DD`."
    )]
    pub async fn inv_schedule_create(
        &self,
        Parameters(input): Parameters<ScheduleCreateWire>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(schedules::create(&self.ctx, input).await)
    }

    /// Pause an active schedule.
    #[tool(
        name = "inv_schedule_pause",
        description = "Pause an active schedule (the ticker will skip it). Idempotent."
    )]
    pub async fn inv_schedule_pause(
        &self,
        Parameters(input): Parameters<ScheduleStateChangeWire>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(schedules::pause(&self.ctx, input).await)
    }

    /// Cancel a schedule (terminal).
    #[tool(
        name = "inv_schedule_cancel",
        description = "Cancel a schedule (terminal). Idempotent for already-cancelled."
    )]
    pub async fn inv_schedule_cancel(
        &self,
        Parameters(input): Parameters<ScheduleStateChangeWire>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(schedules::cancel(&self.ctx, input).await)
    }

    // -----------------------------------------------------------------
    // Reminder tools
    // -----------------------------------------------------------------

    /// Schedule a reminder.
    #[tool(
        name = "inv_reminder_schedule",
        description = "Enqueue a reminder against an invoice. scheduled_at must be in the future."
    )]
    pub async fn inv_reminder_schedule(
        &self,
        Parameters(input): Parameters<ReminderScheduleWire>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(reminders::schedule(&self.ctx, input).await)
    }

    /// Cancel a reminder.
    #[tool(
        name = "inv_reminder_cancel",
        description = "Cancel a scheduled reminder. No-op if already sent or cancelled."
    )]
    pub async fn inv_reminder_cancel(
        &self,
        Parameters(input): Parameters<ReminderCancelWire>,
    ) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(reminders::cancel(&self.ctx, input).await)
    }

    // -----------------------------------------------------------------
    // Ticker tools (parameter-free)
    // -----------------------------------------------------------------

    /// Drain due schedules.
    #[tool(
        name = "inv_tick_schedules",
        description = "Materialise invoices for every active schedule whose next_run <= today."
    )]
    pub async fn inv_tick_schedules(&self) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(tickers::tick_schedules(&self.ctx).await)
    }

    /// Drain due reminders.
    #[tool(
        name = "inv_tick_reminders",
        description = "Dispatch every reminder whose scheduled_at <= now and is still in Scheduled state."
    )]
    pub async fn inv_tick_reminders(&self) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(tickers::tick_reminders(&self.ctx).await)
    }

    /// Flag overdue invoices.
    #[tool(
        name = "inv_tick_overdue",
        description = "Emit `inv.billing.invoice.overdue` for every Issued/Sent/Viewed/PartiallyPaid invoice past its due date."
    )]
    pub async fn inv_tick_overdue(&self) -> Result<rmcp::model::CallToolResult, RmcpError> {
        wrap(tickers::tick_overdue(&self.ctx).await)
    }
}

/// Map an MCP-tool result `Result<serde_json::Value, McpError>` into the
/// rmcp wire-form `Result<CallToolResult, RmcpError>`. Successful
/// payloads land as `CallToolResult::structured(...)`; failures convert
/// to `RmcpError` via the `From<McpError>` impl on
/// [`crate::error::McpError`].
fn wrap(
    res: Result<serde_json::Value, crate::error::McpError>,
) -> Result<rmcp::model::CallToolResult, RmcpError> {
    match res {
        Ok(v) => Ok(rmcp::model::CallToolResult::structured(v)),
        Err(e) => Err(e.into()),
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for InvMcpServer {
    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .enable_resources()
            .build();
        let server_info = rmcp::model::Implementation::new("inv-mcp", env!("CARGO_PKG_VERSION"))
            .with_title("inv MCP server");
        ServerInfo::new(capabilities)
            .with_protocol_version(ProtocolVersion::default())
            .with_server_info(server_info)
            .with_instructions(
                "inv MCP server. Tools expose the inv-commands invoice lifecycle; \
                 resources expose read-only views via inv://<kind>/<id>.",
            )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, RmcpError> {
        // No concrete resources at v1 — clients should use templates +
        // the `read` endpoint with substituted ids. Return an empty
        // list rather than method-not-found.
        Ok(ListResourcesResult::default())
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, RmcpError> {
        let templates = resources::template_list()
            .into_iter()
            .map(|r| {
                // RawResource and RawResourceTemplate share the
                // (uri, name, title, description, mime_type) shape; we
                // rebuild a template-shaped entry from the resource.
                let raw = r.raw;
                use rmcp::model::{AnnotateAble, RawResourceTemplate};
                let mut tmpl = RawResourceTemplate::new(raw.uri, raw.name);
                tmpl.title = raw.title;
                tmpl.description = raw.description;
                tmpl.mime_type = raw.mime_type;
                tmpl.no_annotation()
            })
            .collect();
        Ok(ListResourceTemplatesResult {
            meta: None,
            next_cursor: None,
            resource_templates: templates,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, RmcpError> {
        crate::resources::read(&self.ctx, request)
            .await
            .map_err(|e| e.into())
    }
}
