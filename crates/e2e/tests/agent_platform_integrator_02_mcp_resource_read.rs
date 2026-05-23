//! Story agent-platform-integrator-02: agent reads invoice state via
//! the `inv://invoice/<id>` resource template.
//!
//! Surfaces: MCP (resources/read, resources/list, resource templates).
//!
//! xrr: NOT used. In-process duplex transport, same rationale as story 01.

mod common;

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};

use inv_commands::{Clock, CoreCtx};
use inv_core::tax::TaxTable;
use inv_mcp::InvMcpServer;
use inv_store::pool::Pool;

use rmcp::model::{CallToolRequestParams, ReadResourceRequestParams};
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
async fn resource_read_returns_full_invoice_json() {
    let (ctx, pool) = fresh_mcp_ctx().await;
    let cust = common::seed_customer_qc(&pool).await;
    let client = spawn_pair(ctx).await;

    // Draft via tool so the invoice exists with the right shape.
    let drafted = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_draft").with_arguments(
                serde_json::json!({
                    "customer_id": cust.to_string(),
                    "seller_jurisdiction": "CA-QC",
                    "currency": "CAD",
                    "lines": [
                        { "description": "Consulting", "quantity": "10", "unit_price": "125.00", "tax_category": "standard" }
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

    let resource = client
        .peer()
        .read_resource(ReadResourceRequestParams::new(format!(
            "inv://invoice/{inv_id}"
        )))
        .await
        .expect("read resource");
    assert_eq!(resource.contents.len(), 1);
    let body = match &resource.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => panic!("expected text resource contents, got {other:?}"),
    };
    let parsed: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(parsed["id"], inv_id);
    assert_eq!(parsed["state"], "draft");
    assert!(
        parsed["subtotal"].is_string() || parsed["subtotal"].is_number(),
        "subtotal field present in invoice json"
    );
    // T-0038: the resource read now joins `invoice_lines` into the
    // body. Shape is a structural superset of what `inv_invoice_show`
    // returns: all `Invoice` fields flattened at the top level, plus a
    // `lines` array. Each line carries the bare `InvoiceLine` struct
    // (id, position, description, quantity, unit_price, tax_category,
    // tax_amount, line_total, ...).
    let lines = parsed["lines"]
        .as_array()
        .expect("lines array present in resource body");
    assert_eq!(lines.len(), 1, "seeded one line, got: {lines:?}");
    let line0 = &lines[0];
    assert_eq!(line0["description"], "Consulting");
    assert_eq!(line0["quantity"], "10");
    assert_eq!(line0["unit_price"], "125.00");
    assert!(
        line0["line_total"].is_string(),
        "line_total decimal-string present, got: {:?}",
        line0["line_total"]
    );

    // Sanity: line_totals already include per-line tax, so the sum
    // matches invoice total (not subtotal). Subtotal is the sum of
    // (quantity * unit_price) before tax — verify both invariants on the
    // joined payload to catch any drift between the resource shape and
    // the underlying repo data.
    let line_total_sum: rust_decimal::Decimal = lines
        .iter()
        .map(|l| {
            l["line_total"]
                .as_str()
                .expect("line_total str")
                .parse::<rust_decimal::Decimal>()
                .expect("line_total decimal")
        })
        .sum();
    let invoice_total: rust_decimal::Decimal = parsed["total"]
        .as_str()
        .expect("total str")
        .parse()
        .expect("total decimal");
    assert_eq!(
        line_total_sum, invoice_total,
        "sum(line_total) must equal invoice.total"
    );
    let qty_x_price_sum: rust_decimal::Decimal = lines
        .iter()
        .map(|l| {
            let q: rust_decimal::Decimal = l["quantity"]
                .as_str()
                .expect("quantity str")
                .parse()
                .expect("quantity decimal");
            let p: rust_decimal::Decimal = l["unit_price"]
                .as_str()
                .expect("unit_price str")
                .parse()
                .expect("unit_price decimal");
            q * p
        })
        .sum();
    let invoice_subtotal: rust_decimal::Decimal = parsed["subtotal"]
        .as_str()
        .expect("subtotal str")
        .parse()
        .expect("subtotal decimal");
    assert_eq!(
        qty_x_price_sum, invoice_subtotal,
        "sum(quantity * unit_price) must equal invoice.subtotal"
    );
}

#[tokio::test]
async fn resource_list_is_empty_per_v1_design() {
    let (ctx, _pool) = fresh_mcp_ctx().await;
    let client = spawn_pair(ctx).await;
    let listed = client.peer().list_resources(None).await.expect("list");
    assert!(
        listed.resources.is_empty(),
        "v1 behaviour: resources/list returns empty (templates only)"
    );
}

#[tokio::test]
async fn resource_templates_advertise_invoice_uri() {
    let (ctx, _pool) = fresh_mcp_ctx().await;
    let client = spawn_pair(ctx).await;
    let tpls = client
        .peer()
        .list_resource_templates(None)
        .await
        .expect("list templates");
    let uris: Vec<&str> = tpls
        .resource_templates
        .iter()
        .map(|t| t.uri_template.as_str())
        .collect();
    assert!(
        uris.iter().any(|u| u.starts_with("inv://invoice/")),
        "missing inv://invoice/ template: {uris:?}"
    );
}

#[tokio::test]
async fn resource_read_unknown_id_errors() {
    let (ctx, _pool) = fresh_mcp_ctx().await;
    let client = spawn_pair(ctx).await;

    // A well-formed-but-nonexistent invoice id.
    let id = inv_core::domain::ids::InvoiceId::new();
    let err = client
        .peer()
        .read_resource(ReadResourceRequestParams::new(format!(
            "inv://invoice/{id}"
        )))
        .await
        .expect_err("unknown id should error");
    let msg = format!("{err}").to_ascii_lowercase();
    assert!(
        msg.contains("not") || msg.contains("invoice") || msg.contains("error"),
        "expected not-found-like error, got: {err}"
    );
}
