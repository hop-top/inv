//! Story agent-platform-integrator-01: MCP `inv_invoice_draft` exposes
//! a stable JsonSchema; the tool round-trips with HTTP/CLI shape.
//!
//! Surfaces: MCP (tools/list + tools/call via in-process duplex peer).
//!
//! xrr: NOT used. MCP runs in-process over a tokio::io::duplex pipe
//! (same as crates/mcp/tests/integration.rs). No subprocess, no
//! external boundary. xrr would be overhead per the triage.

mod common;

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};

use inv_commands::{Clock, CoreCtx};
use inv_core::tax::TaxTable;
use inv_mcp::InvMcpServer;
use inv_store::pool::Pool;

use rmcp::model::CallToolRequestParams;
use rmcp::service::ServiceExt;

struct FrozenClock(DateTime<Utc>);
impl Clock for FrozenClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}
fn frozen_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 19, 12, 0, 0).unwrap()
}

async fn fresh_mcp_ctx() -> (Arc<CoreCtx>, Pool) {
    let pool = common::fresh_pool().await;
    let (table, nexus) = TaxTable::load_from_str(common::FIXTURE_TOML).expect("tax");
    let ctx =
        CoreCtx::new(pool.clone(), table, nexus).with_clock(Arc::new(FrozenClock(frozen_now())));
    (Arc::new(ctx), pool)
}

async fn spawn_pair(ctx: Arc<CoreCtx>) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let (server_t, client_t) = tokio::io::duplex(64 * 1024);
    let server = InvMcpServer::new(ctx);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_t).await {
            let _ = running.waiting().await;
        }
    });
    ().serve(client_t).await.expect("client serve")
}

#[tokio::test]
async fn inv_invoice_draft_advertises_stable_input_schema() {
    let (ctx, _pool) = fresh_mcp_ctx().await;
    let client = spawn_pair(ctx).await;

    let tools = client.peer().list_tools(None).await.expect("list_tools");
    let names: Vec<&str> = tools.tools.iter().map(|t| t.name.as_ref()).collect();
    assert!(
        names.contains(&"inv_invoice_draft"),
        "missing inv_invoice_draft, have {names:?}"
    );

    let draft_tool = tools
        .tools
        .iter()
        .find(|t| t.name.as_ref() == "inv_invoice_draft")
        .expect("draft tool");

    // The inputSchema is a `Map<String, Value>` (rmcp wraps it). Round-trip
    // it through serde_json::Value to validate the shape documented in
    // reference/mcp.md.
    let schema_json = serde_json::to_value(&draft_tool.input_schema).expect("schema → json");
    // Two valid shapes: a top-level $ref into definitions, or an inline
    // schema with a `properties` map. Either way, the required fields
    // should be derivable; assert by re-rendering + searching.
    let s = serde_json::to_string(&schema_json).expect("re-render");
    for field in ["customer_id", "currency", "lines"] {
        assert!(
            s.contains(&format!("\"{field}\"")),
            "schema missing field `{field}`: {s}"
        );
    }
    // Snake_case enforced.
    assert!(
        !s.contains("\"customerId\""),
        "schema should use snake_case, not camelCase",
    );
}

#[tokio::test]
async fn inv_invoice_draft_happy_path_call() {
    let (ctx, pool) = fresh_mcp_ctx().await;
    // Seed a customer through the pool.
    let cust = common::seed_customer_qc(&pool).await;
    let client = spawn_pair(ctx).await;

    let out = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_draft").with_arguments(
                serde_json::json!({
                    "customer_id": cust.to_string(),
                    "seller_jurisdiction": "CA-QC",
                    "currency": "CAD",
                    "lines": [
                        {
                            "description": "Consulting",
                            "quantity": "10",
                            "unit_price": "125.00",
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
        .expect("call_tool");
    let sc = out.structured_content.as_ref().expect("structured content");
    let inv_id = sc
        .pointer("/invoice/id")
        .and_then(|v| v.as_str())
        .expect("id");
    assert!(inv_id.starts_with("invoice_"), "got `{inv_id}`");
    // emitted_events surfaced — see story-03 for the full triplet check;
    // here we just confirm the field is present.
    assert!(
        sc.pointer("/emitted_events").is_some(),
        "tool result should include emitted_events: {sc}"
    );
}

#[tokio::test]
async fn inv_invoice_show_returns_lines_matching_resource_shape() {
    // T-0041: `inv_invoice_show` is now structurally identical to the
    // `inv://invoice/<id>` resource — bare `Invoice` fields flattened
    // at the top level plus a `lines` array. An agent that already
    // knows the resource shape should be able to switch to the tool
    // without code changes.
    let (ctx, pool) = fresh_mcp_ctx().await;
    let cust = common::seed_customer_qc(&pool).await;
    let client = spawn_pair(ctx).await;

    let drafted = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_draft").with_arguments(
                serde_json::json!({
                    "customer_id": cust.to_string(),
                    "seller_jurisdiction": "CA-QC",
                    "currency": "CAD",
                    "lines": [
                        { "description": "Consulting", "quantity": "10", "unit_price": "125.00", "tax_category": "standard" },
                        { "description": "Hosting",    "quantity": "1",  "unit_price": "50.00",  "tax_category": "standard" }
                    ]
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect("draft");
    let inv_id = drafted
        .structured_content
        .as_ref()
        .and_then(|v| v.pointer("/invoice/id"))
        .and_then(|v| v.as_str())
        .expect("inv id")
        .to_string();

    let shown = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_show").with_arguments(
                serde_json::json!({ "invoice_id": inv_id })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("show");
    let sc = shown
        .structured_content
        .as_ref()
        .expect("structured content");

    // Top-level invoice fields (flattened).
    assert_eq!(sc["id"], inv_id);
    assert_eq!(sc["state"], "draft");
    // Joined lines.
    let lines = sc["lines"].as_array().expect("lines array");
    assert_eq!(
        lines.len(),
        2,
        "show should return both lines, got: {lines:?}"
    );
    assert_eq!(lines[0]["description"], "Consulting");
    assert_eq!(lines[1]["description"], "Hosting");
    assert_eq!(lines[0]["position"], 0);
    assert_eq!(lines[1]["position"], 1);
}

#[tokio::test]
async fn inv_invoice_draft_validation_error_surfaces_mcp_error() {
    let (ctx, _pool) = fresh_mcp_ctx().await;
    let client = spawn_pair(ctx).await;

    // Empty `lines` -> validation error at the command layer.
    let err = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_draft").with_arguments(
                serde_json::json!({
                    "customer_id": "customer_definitely_invalid_typeid",
                    "seller_jurisdiction": "CA-QC",
                    "currency": "CAD",
                    "lines": []
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .expect_err("expected MCP error");
    let msg = format!("{err}");
    assert!(
        msg.to_ascii_lowercase().contains("validation")
            || msg.to_ascii_lowercase().contains("invalid")
            || msg.contains("customer_id")
            || msg.contains("lines"),
        "unexpected error: {msg}"
    );
}
