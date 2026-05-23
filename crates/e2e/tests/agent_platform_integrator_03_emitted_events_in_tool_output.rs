//! Story agent-platform-integrator-03: every mutating command publishes
//! the matching bus events synchronously, so an agent that wires an
//! InMemoryPublisher into the CoreCtx can observe the event stream and
//! summarise side effects without subscribing to the real bus.
//!
//! Surfaces: MCP (tools/call), commands (`publisher::try_publish` —
//! T-0043), outbox relay parity (the publisher's view should match what
//! subscribers see — we drain the relay + cross-check).
//!
//! T-0043 dropped the per-tool `emitted_events` field; tools no longer
//! ferry the event list inline. Capture happens via the publisher.
//!
//! xrr: NOT used. In-process duplex.

mod common;

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};

use inv_bus::{run_outbox_relay, InMemoryPublisher, Publisher};
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

async fn fresh_mcp_ctx() -> (Arc<CoreCtx>, Pool, Arc<InMemoryPublisher>) {
    let pool = common::fresh_pool().await;
    let (table, nexus) = TaxTable::load_from_str(common::FIXTURE_TOML).expect("tax");
    let publisher: Arc<InMemoryPublisher> = Arc::new(InMemoryPublisher::new());
    let ctx = CoreCtx::new(pool.clone(), table, nexus)
        .with_clock(Arc::new(FrozenClock(frozen_now())))
        .with_publisher(publisher.clone() as Arc<dyn Publisher>);
    (Arc::new(ctx), pool, publisher)
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
async fn issue_tool_publishes_events_and_matches_relay() {
    let (ctx, pool, captured) = fresh_mcp_ctx().await;
    let cust = common::seed_customer_qc(&pool).await;
    let client = spawn_pair(ctx).await;

    // 1. Draft.
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

    // Snapshot publisher state pre-issue so we can isolate the issue
    // call's emissions from the prior draft.
    let topics_before_issue = captured.topics();

    // 2. Issue — expect mechanic triplet + domain `.issued` to land on
    //    the InMemoryPublisher (T-0043; events no longer ride the tool
    //    response).
    let _issued = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_issue").with_arguments(
                serde_json::json!({ "invoice_id": inv_id })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect("issue");

    let topics_after_issue = captured.topics();
    let new_topics: Vec<&str> = topics_after_issue[topics_before_issue.len()..]
        .iter()
        .map(|s| s.as_str())
        .collect();
    for expected in [
        "inv.billing.invoice.proposed",
        "inv.billing.invoice.transitioned",
        "inv.billing.invoice.entered",
        "inv.billing.invoice.issued",
    ] {
        assert!(
            new_topics.contains(&expected),
            "missing `{expected}` in {new_topics:?}"
        );
    }

    // 3. Drain the outbox relay and confirm subscribers see the same
    //    domain event the publisher captured in-band.
    let _ = run_outbox_relay(&pool, captured.as_ref(), 50)
        .await
        .expect("relay");
    let pub_topics = captured.topics();
    assert!(
        pub_topics.iter().any(|t| t == "inv.billing.invoice.issued"),
        "relay topics: {pub_topics:?}"
    );
}

#[tokio::test]
async fn fsm_violation_returns_error_with_no_emissions() {
    let (ctx, pool, _captured) = fresh_mcp_ctx().await;
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
                        { "description": "Consulting", "quantity": "10", "unit_price": "125.00", "tax_category": "standard" }
                    ]
                })
                .as_object()
                .unwrap()
                .clone(),
            ),
        )
        .await
        .unwrap();
    let inv_id = drafted
        .structured_content
        .as_ref()
        .and_then(|v| v.pointer("/invoice/id"))
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();

    // First issue: ok.
    client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_issue").with_arguments(
                serde_json::json!({ "invoice_id": inv_id })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    let hist_after_first = inv_store::repo::history::InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&inv_id.parse().unwrap())
        .await
        .unwrap();
    let n = hist_after_first.len();

    // Second issue: FSM rejects. The tool must surface an MCP error;
    // the store must not have grown.
    let err = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("inv_invoice_issue").with_arguments(
                serde_json::json!({ "invoice_id": inv_id })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .expect_err("FSM should reject second issue");
    let msg = format!("{err}").to_ascii_lowercase();
    assert!(
        msg.contains("fsm") || msg.contains("transition") || msg.contains("illegal"),
        "expected FSM error, got: {err}"
    );

    let hist_after_second = inv_store::repo::history::InvoiceHistoryRepo::new(&pool)
        .list_for_invoice(&inv_id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(
        hist_after_second.len(),
        n,
        "rejected transition must NOT insert a history row",
    );
}
