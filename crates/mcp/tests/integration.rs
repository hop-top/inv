//! Integration tests for the inv-mcp adapter.
//!
//! Spins up an in-memory MCP client connected to an
//! [`InvMcpServer`][inv_mcp::InvMcpServer] over a tokio duplex pipe.
//! The client side is rmcp's default `()`-based `ClientHandler`. The
//! server side is backed by an in-memory sqlite [`CoreCtx`] with the
//! same QC tax-fixture used by inv-commands tests.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};

use inv_commands::{Clock, CoreCtx};
use inv_core::domain::address::Address;
use inv_core::domain::customer::Customer;
use inv_core::domain::ids::CustomerId;
use inv_core::tax::TaxTable;
use inv_mcp::InvMcpServer;
use inv_store::pool::{connect, Pool};
use inv_store::repo::customer::CustomerRepo;
use inv_store::run_migrations;

use rmcp::model::{CallToolRequestParams, ReadResourceRequestParams};
use rmcp::service::ServiceExt;

const FIXTURE_TOML: &str = r#"
[[rate]]
id = "ca-qc-gst"
jurisdiction = "CA-QC"
applies_to_buyer = { country = "CA" }
name = "GST"
category = "standard"
rate = "0.05"
effective_from = "2008-01-01"

[[rate]]
id = "ca-qc-qst"
jurisdiction = "CA-QC"
applies_to_buyer = { country = "CA", region = "QC" }
name = "QST"
category = "standard"
rate = "0.09975"
effective_from = "2013-01-01"
"#;

struct FrozenClock(DateTime<Utc>);

impl Clock for FrozenClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

fn frozen_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap()
}

async fn fresh_ctx() -> (Arc<CoreCtx>, Pool) {
    let pool = connect("sqlite::memory:").await.expect("connect");
    run_migrations(&pool).await.expect("migrate");
    let (table, nexus) = TaxTable::load_from_str(FIXTURE_TOML).expect("tax fixture");
    let ctx =
        CoreCtx::new(pool.clone(), table, nexus).with_clock(Arc::new(FrozenClock(frozen_now())));
    (Arc::new(ctx), pool)
}

async fn seed_customer(pool: &Pool) -> CustomerId {
    let c = Customer {
        id: CustomerId::new(),
        display_name: "Acme Corp".into(),
        email: Some("billing@acme.example".into()),
        address: Address {
            country: "CA".into(),
            region: Some("QC".into()),
            city: Some("Montréal".into()),
            postal: None,
            line1: None,
            line2: None,
        },
        metadata: BTreeMap::new(),
        created_at: frozen_now(),
        updated_at: frozen_now(),
    };
    CustomerRepo::new(pool)
        .save(&c)
        .await
        .expect("seed customer");
    c.id
}

/// Spawn the server side on a duplex transport. Returns the connected
/// client peer.
async fn spawn_pair(ctx: Arc<CoreCtx>) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);

    // Drive the server on a background task. We deliberately don't
    // hold onto the returned `RunningService` join handle here — the
    // task will end when the client closes the transport.
    let server = InvMcpServer::new(ctx);
    tokio::spawn(async move {
        let running = server
            .serve(server_transport)
            .await
            .expect("server serve init");
        let _ = running.waiting().await;
    });

    // `()` implements rmcp::ClientHandler with sensible defaults.
    let client = ().serve(client_transport).await.expect("client serve");
    client
}

#[tokio::test]
async fn lists_at_least_18_tools() {
    let (ctx, _pool) = fresh_ctx().await;
    let client = spawn_pair(ctx).await;

    let tools = client.peer().list_tools(None).await.expect("list_tools");
    assert!(
        tools.tools.len() >= 18,
        "expected ≥18 tools, got {}: {:?}",
        tools.tools.len(),
        tools
            .tools
            .iter()
            .map(|t| t.name.as_ref())
            .collect::<Vec<_>>()
    );

    // Spot-check a few names from the design §10 column.
    let names: Vec<&str> = tools.tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in [
        "inv_invoice_draft",
        "inv_invoice_issue",
        "inv_invoice_pay",
        "inv_invoice_void",
        "inv_invoice_show",
        "inv_invoice_list",
        "inv_creditnote_draft",
        "inv_creditnote_issue",
        "inv_schedule_create",
        "inv_schedule_pause",
        "inv_schedule_cancel",
        "inv_reminder_schedule",
        "inv_reminder_cancel",
        "inv_tick_schedules",
        "inv_tick_reminders",
        "inv_tick_overdue",
        "inv_customer_add",
        "inv_customer_show",
        "inv_customer_list",
        "inv_tax_rates_show",
    ] {
        assert!(
            names.contains(&expected),
            "missing tool `{expected}`; have {names:?}"
        );
    }
}

#[tokio::test]
async fn full_lifecycle_via_mcp() {
    let (ctx, pool) = fresh_ctx().await;
    let cust_id = seed_customer(&pool).await;
    let client = spawn_pair(ctx).await;

    // 1. Draft an invoice via `inv_invoice_draft`.
    let draft = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_draft").with_arguments(
                serde_json::json!({
                    "customer_id": cust_id.to_string(),
                    "seller_jurisdiction": "CA-QC",
                    "currency": "CAD",
                    "lines": [
                        {
                            "description": "Consulting",
                            "quantity": "10",
                            "unit_price": "100",
                            "tax_category": "standard"
                        }
                    ]
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("draft");
    let draft_structured = draft
        .structured_content
        .as_ref()
        .expect("draft returns structured content");
    let invoice_id = draft_structured
        .pointer("/invoice/id")
        .and_then(|v| v.as_str())
        .expect("invoice id")
        .to_string();
    assert!(
        invoice_id.starts_with("invoice_"),
        "MTI prefix `invoice_`, got `{invoice_id}`"
    );

    // 2. Issue it via `inv_invoice_issue`.
    let issued = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_issue").with_arguments(
                serde_json::json!({ "invoice_id": invoice_id })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("issue");
    let issued_state = issued
        .structured_content
        .as_ref()
        .and_then(|v| v.pointer("/invoice/state"))
        .and_then(|v| v.as_str())
        .expect("issued state");
    assert_eq!(issued_state, "issued", "post-issue state must be `issued`");

    // 3. Pay it via `inv_invoice_pay`.
    let paid = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_pay").with_arguments(
                serde_json::json!({
                    "invoice_id": invoice_id,
                    "amount": "10000",
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("pay");
    let fully_paid = paid
        .structured_content
        .as_ref()
        .and_then(|v| v.pointer("/fully_paid"))
        .and_then(|v| v.as_bool())
        .expect("fully_paid flag");
    assert!(fully_paid, "10000 > total → fully paid");

    // 4. Read the invoice resource.
    let resource = client
        .peer()
        .read_resource(ReadResourceRequestParams::new(format!(
            "inv://invoice/{invoice_id}"
        )))
        .await
        .expect("read resource");
    assert_eq!(resource.contents.len(), 1);
    let body = match &resource.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        _ => panic!("expected text resource contents"),
    };
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("json body");
    assert_eq!(parsed["state"], "paid");
    assert_eq!(parsed["id"], invoice_id);
}

#[tokio::test]
async fn invalid_tool_input_surfaces_mcp_error() {
    let (ctx, _pool) = fresh_ctx().await;
    let client = spawn_pair(ctx).await;

    // Draft with an empty lines array → CoreError::Validation → MCP
    // invalid-params error.
    let err = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_draft").with_arguments(
                serde_json::json!({
                    "customer_id": "cust_definitely_invalid",
                    "seller_jurisdiction": "CA-QC",
                    "currency": "CAD",
                    "lines": []
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await;
    let err = err.expect_err("expected MCP error on bad input");
    let msg = format!("{err}");
    assert!(
        msg.contains("customer_id") || msg.contains("invalid") || msg.contains("validation"),
        "unhelpful err: {msg}"
    );
}

#[tokio::test]
async fn resource_read_includes_lines_single() {
    // T-0038: `inv://invoice/<id>` joins `invoice_lines` into the
    // payload. Single-line invoice → `lines` array of length 1.
    let (ctx, pool) = fresh_ctx().await;
    let cust_id = seed_customer(&pool).await;
    let client = spawn_pair(ctx).await;

    let draft = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_draft").with_arguments(
                serde_json::json!({
                    "customer_id": cust_id.to_string(),
                    "seller_jurisdiction": "CA-QC",
                    "currency": "CAD",
                    "lines": [
                        {
                            "description": "Consulting",
                            "quantity": "10",
                            "unit_price": "100",
                            "tax_category": "standard"
                        }
                    ]
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("draft");
    let invoice_id = draft
        .structured_content
        .as_ref()
        .and_then(|v| v.pointer("/invoice/id"))
        .and_then(|v| v.as_str())
        .expect("invoice id")
        .to_string();

    let resource = client
        .peer()
        .read_resource(ReadResourceRequestParams::new(format!(
            "inv://invoice/{invoice_id}"
        )))
        .await
        .expect("read resource");
    let body = match &resource.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        _ => panic!("text contents"),
    };
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("json");
    // Invoice fields flattened at top level.
    assert_eq!(parsed["id"], invoice_id);
    assert_eq!(parsed["state"], "draft");
    // Lines joined.
    let lines = parsed["lines"].as_array().expect("lines array");
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["description"], "Consulting");
    assert_eq!(lines[0]["quantity"], "10");
    assert_eq!(lines[0]["unit_price"], "100");
    assert_eq!(lines[0]["position"], 0);
    assert!(lines[0]["line_total"].is_string(), "line_total decimal-str");
    assert!(lines[0]["tax_amount"].is_string(), "tax_amount decimal-str");
    // Bare InvoiceLine struct fields — `id` + `invoice_id` present.
    assert!(lines[0]["id"].is_string(), "line id present");
    assert_eq!(lines[0]["invoice_id"], invoice_id);
}

#[tokio::test]
async fn resource_read_includes_lines_multi_in_position_order() {
    // T-0038: multi-line invoice — verify `lines` ordering follows
    // `position ASC` (the repo guarantee) and that every line is
    // included.
    let (ctx, pool) = fresh_ctx().await;
    let cust_id = seed_customer(&pool).await;
    let client = spawn_pair(ctx).await;

    let draft = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_draft").with_arguments(
                serde_json::json!({
                    "customer_id": cust_id.to_string(),
                    "seller_jurisdiction": "CA-QC",
                    "currency": "CAD",
                    "lines": [
                        { "description": "Design",      "quantity": "1",  "unit_price": "200", "tax_category": "standard" },
                        { "description": "Consulting", "quantity": "10", "unit_price": "150", "tax_category": "standard" },
                        { "description": "Hosting",    "quantity": "12", "unit_price": "25",  "tax_category": "standard" }
                    ]
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("draft");
    let invoice_id = draft
        .structured_content
        .as_ref()
        .and_then(|v| v.pointer("/invoice/id"))
        .and_then(|v| v.as_str())
        .expect("invoice id")
        .to_string();

    let resource = client
        .peer()
        .read_resource(ReadResourceRequestParams::new(format!(
            "inv://invoice/{invoice_id}"
        )))
        .await
        .expect("read resource");
    let body = match &resource.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        _ => panic!("text contents"),
    };
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("json");
    let lines = parsed["lines"].as_array().expect("lines array");
    assert_eq!(lines.len(), 3);
    let descs: Vec<&str> = lines
        .iter()
        .map(|l| l["description"].as_str().unwrap())
        .collect();
    assert_eq!(descs, vec!["Design", "Consulting", "Hosting"]);
    let positions: Vec<u64> = lines
        .iter()
        .map(|l| l["position"].as_u64().expect("position int"))
        .collect();
    assert_eq!(positions, vec![0, 1, 2], "position must be 0-based dense");
}

#[tokio::test]
async fn resource_read_unknown_invoice_returns_error() {
    // T-0038: a well-formed-but-nonexistent id maps to McpError::NotFound
    // (rmcp invalid_params per error.rs).
    let (ctx, _pool) = fresh_ctx().await;
    let client = spawn_pair(ctx).await;
    let bogus = inv_core::domain::ids::InvoiceId::new();
    let err = client
        .peer()
        .read_resource(ReadResourceRequestParams::new(format!(
            "inv://invoice/{bogus}"
        )))
        .await
        .expect_err("unknown id must error");
    let msg = format!("{err}").to_ascii_lowercase();
    assert!(
        msg.contains("not") || msg.contains("invoice") || msg.contains("found"),
        "unhelpful err: {err}"
    );
}

#[tokio::test]
async fn show_tool_and_resource_return_identical_shape() {
    // T-0041: `inv_invoice_show` and `inv://invoice/<id>` MUST produce
    // byte-for-byte identical JSON (post-parse Value equality — the
    // resource pretty-prints text whereas the tool returns structured
    // content, so equality is checked at the parsed `Value` level).
    let (ctx, pool) = fresh_ctx().await;
    let cust_id = seed_customer(&pool).await;
    let client = spawn_pair(ctx).await;

    // Seed a multi-line invoice so `lines` is non-trivial.
    let draft = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_draft").with_arguments(
                serde_json::json!({
                    "customer_id": cust_id.to_string(),
                    "seller_jurisdiction": "CA-QC",
                    "currency": "CAD",
                    "lines": [
                        { "description": "Design",     "quantity": "1",  "unit_price": "200", "tax_category": "standard" },
                        { "description": "Consulting", "quantity": "10", "unit_price": "150", "tax_category": "standard" },
                        { "description": "Hosting",    "quantity": "12", "unit_price": "25",  "tax_category": "standard" }
                    ]
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("draft");
    let invoice_id = draft
        .structured_content
        .as_ref()
        .and_then(|v| v.pointer("/invoice/id"))
        .and_then(|v| v.as_str())
        .expect("invoice id")
        .to_string();

    // Tool side: `inv_invoice_show` → structured_content.
    let shown = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_show").with_arguments(
                serde_json::json!({ "invoice_id": invoice_id })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("show");
    let tool_value: serde_json::Value = shown
        .structured_content
        .as_ref()
        .expect("show returns structured content")
        .clone();

    // Resource side: `inv://invoice/<id>` → pretty JSON text.
    let resource = client
        .peer()
        .read_resource(ReadResourceRequestParams::new(format!(
            "inv://invoice/{invoice_id}"
        )))
        .await
        .expect("read resource");
    let body = match &resource.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        _ => panic!("text contents"),
    };
    let resource_value: serde_json::Value =
        serde_json::from_str(&body).expect("resource json parse");

    // Byte-for-byte parity post-parse: identical `Value` trees.
    assert_eq!(
        tool_value, resource_value,
        "inv_invoice_show output must equal inv://invoice/{{id}} body\n\
         tool: {tool_value}\n\
         resource: {resource_value}"
    );

    // Cross-check both sides actually carry the joined `lines` (defends
    // against the case where both happen to be wrong in the same way —
    // e.g. both regressing to bare Invoice).
    let tool_lines = tool_value["lines"].as_array().expect("tool lines array");
    assert_eq!(tool_lines.len(), 3, "tool lines: {tool_lines:?}");
    assert_eq!(tool_value["id"], invoice_id);
    assert_eq!(tool_value["state"], "draft");
}

#[tokio::test]
async fn resource_templates_advertise_five_kinds() {
    let (ctx, _pool) = fresh_ctx().await;
    let client = spawn_pair(ctx).await;

    let templates = client
        .peer()
        .list_resource_templates(None)
        .await
        .expect("list templates");
    assert_eq!(
        templates.resource_templates.len(),
        5,
        "templates: {:?}",
        templates
            .resource_templates
            .iter()
            .map(|t| t.uri_template.as_str())
            .collect::<Vec<_>>()
    );
}
