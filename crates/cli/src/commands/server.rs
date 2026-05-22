//! `inv server` — long-lived process that composes every channel.
//!
//! Boots the HTTP API + WebSocket on a single axum listener, drives the
//! three internal tickers (schedules, reminders, overdue), runs the
//! outbox relay loop, and optionally serves MCP over stdio. Waits on
//! SIGTERM/SIGINT for graceful shutdown.
//!
//! ## Topology
//!
//! ```text
//!   inv server
//!     │
//!     ├─► HTTP+WS server (axum on 127.0.0.1:7400 by default)
//!     │     ├─► /v1/*    (inv-api routes, bearer-auth-gated)
//!     │     ├─► /v/:token (signed-link view, public)
//!     │     ├─► /ws        (inv-ws upgrade)
//!     │     └─► /healthz
//!     │
//!     ├─► outbox relay  (periodic, drains invoice_state_history +
//!     │                  credit_note_state_history → bus publisher)
//!     │
//!     ├─► tickers
//!     │     ├─► schedules_tick   (default daily)
//!     │     ├─► reminders_tick   (default every 5 min)
//!     │     └─► mark_overdue_ticker (default hourly)
//!     │
//!     └─► MCP stdio (optional, --mcp; exclusive with stdout logging)
//! ```
//!
//! MCP-over-stdio is exclusive because the JSON-RPC stream shares the
//! process's stdout. When `--mcp` is set, we route tracing to a file
//! sink instead of stdout.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use clap::{ArgMatches, Command};
use inv_api::{ApiConfig, ApiState};
use inv_commands::{mark_overdue_ticker, reminders_tick, schedules_tick, CoreCtx};
use tokio::net::TcpListener;
use tokio::signal;
use tokio::time::interval;

/// Build the clap subcommand for `inv server`.
pub fn command() -> Command {
    Command::new("server")
        .about("Run the long-lived inv process (HTTP + WS + tickers + outbox)")
        .arg(
            clap::Arg::new("listen")
                .long("listen")
                .default_value("127.0.0.1:7400")
                .help("Address to bind the HTTP + WS server on"),
        )
        .arg(
            clap::Arg::new("mcp")
                .long("mcp")
                .action(clap::ArgAction::SetTrue)
                .help("Serve MCP over stdio (exclusive: silences stdout tracing)"),
        )
        .arg(
            clap::Arg::new("outbox-interval-secs")
                .long("outbox-interval-secs")
                .default_value("5")
                .help("How often to drain the bus outbox"),
        )
        .arg(
            clap::Arg::new("schedules-interval-secs")
                .long("schedules-interval-secs")
                .default_value("86400")
                .help("How often to materialise recurring schedules (default: daily)"),
        )
        .arg(
            clap::Arg::new("reminders-interval-secs")
                .long("reminders-interval-secs")
                .default_value("300")
                .help("How often to dispatch reminders (default: every 5 min)"),
        )
        .arg(
            clap::Arg::new("overdue-interval-secs")
                .long("overdue-interval-secs")
                .default_value("3600")
                .help("How often to flag overdue invoices (default: hourly)"),
        )
}

/// Dispatch `inv server`.
pub async fn dispatch(ctx: &CoreCtx, m: &ArgMatches) -> anyhow::Result<()> {
    let listen: &String = m.get_one("listen").unwrap();
    let mcp = m.get_flag("mcp");
    let outbox_secs: u64 = parse_secs(m, "outbox-interval-secs")?;
    let schedules_secs: u64 = parse_secs(m, "schedules-interval-secs")?;
    let reminders_secs: u64 = parse_secs(m, "reminders-interval-secs")?;
    let overdue_secs: u64 = parse_secs(m, "overdue-interval-secs")?;

    tracing::info!(addr = %listen, mcp, "inv server starting");

    // ----- Build adapter routers ----------------------------------------
    // Outbox relay publisher: writes every drained event to tracing.
    let publisher: Arc<inv_bus::LoggingPublisher> = Arc::new(inv_bus::LoggingPublisher);

    // WS + synchronous-publish publisher: a single broadcast channel so
    // the api `/v/{token}` view route's synchronous publish (T-0031),
    // every per-connection ws subscriber, and the outbox relay all see
    // the same event stream.
    let ws_publisher: inv_ws::SharedPublisher = Arc::new(inv_bus::BroadcastPublisher::new(1024));

    // Attach the broadcast publisher to CoreCtx so view-route emits land
    // immediately. The history outbox stays the canonical delivery path
    // — synchronous publish is a real-time fanout shortcut.
    let mut ctx_with_pub = ctx.clone();
    ctx_with_pub.publisher = Some(ws_publisher.clone() as Arc<dyn inv_commands::Publisher>);
    let ctx_arc = Arc::new(ctx_with_pub);

    let api_state = Arc::new(ApiState::new(
        ctx_arc.clone(),
        ApiConfig::default(),
    ));
    let api_router = inv_api::router(api_state);

    let ws_router = inv_ws::router(ctx_arc.clone(), ws_publisher);

    let app = api_router.merge(ws_router);

    // ----- Bind listener ------------------------------------------------
    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    let local = listener.local_addr().context("local_addr")?;
    tracing::info!(addr = %local, "HTTP + WS listening");

    // ----- Spawn server + tickers + outbox ------------------------------
    let server_handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "axum server exited");
        }
    });

    let outbox_handle = spawn_outbox_loop(ctx_arc.clone(), publisher.clone(), outbox_secs);
    let schedules_handle = spawn_schedules_loop(ctx_arc.clone(), schedules_secs);
    let reminders_handle = spawn_reminders_loop(ctx_arc.clone(), reminders_secs);
    let overdue_handle = spawn_overdue_loop(ctx_arc.clone(), overdue_secs);

    let mcp_handle = if mcp {
        let ctx_mcp = ctx_arc.clone();
        Some(tokio::spawn(async move {
            if let Err(e) = inv_mcp::run_stdio(ctx_mcp).await {
                tracing::error!(error = %e, "mcp stdio exited");
            }
        }))
    } else {
        None
    };

    // ----- Wait on shutdown signal -------------------------------------
    wait_shutdown().await?;
    tracing::info!("shutdown signal received; stopping background tasks");

    // Abort spawned tasks. axum 0.8 doesn't expose graceful shutdown
    // through `serve` in this minimal config; aborting drops in-flight
    // connections. T-0029-style follow-up: route a shutdown channel
    // into axum::serve_with_graceful_shutdown.
    server_handle.abort();
    outbox_handle.abort();
    schedules_handle.abort();
    reminders_handle.abort();
    overdue_handle.abort();
    if let Some(h) = mcp_handle {
        h.abort();
    }

    Ok(())
}

fn parse_secs(m: &ArgMatches, key: &str) -> anyhow::Result<u64> {
    let raw: &String = m
        .get_one(key)
        .ok_or_else(|| anyhow!("missing --{key}"))?;
    raw.parse()
        .with_context(|| format!("parsing --{key} as u64 seconds: {raw}"))
}

fn spawn_outbox_loop(
    ctx: Arc<CoreCtx>,
    publisher: Arc<inv_bus::LoggingPublisher>,
    interval_secs: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(interval_secs));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match inv_bus::run_outbox_relay(&ctx.db, publisher.as_ref(), 100).await {
                Ok(stats) => {
                    if stats.events_published > 0 {
                        tracing::info!(
                            rows = stats.rows_processed,
                            events = stats.events_published,
                            "outbox relay tick"
                        );
                    }
                }
                Err(e) => tracing::error!(error = %e, "outbox relay tick failed"),
            }
        }
    })
}

fn spawn_schedules_loop(ctx: Arc<CoreCtx>, interval_secs: u64) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(interval_secs));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match schedules_tick(&ctx).await {
                Ok(out) => {
                    if !out.ran_schedule_ids.is_empty() {
                        tracing::info!(
                            ran = out.ran_schedule_ids.len(),
                            drafts = out.drafts.len(),
                            "schedules tick"
                        );
                    }
                }
                Err(e) => tracing::error!(error = %e, "schedules tick failed"),
            }
        }
    })
}

fn spawn_reminders_loop(ctx: Arc<CoreCtx>, interval_secs: u64) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(interval_secs));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match reminders_tick(&ctx).await {
                Ok(out) => {
                    if !out.sent_reminders.is_empty() {
                        tracing::info!(sent = out.sent_reminders.len(), "reminders tick");
                    }
                }
                Err(e) => tracing::error!(error = %e, "reminders tick failed"),
            }
        }
    })
}

fn spawn_overdue_loop(ctx: Arc<CoreCtx>, interval_secs: u64) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_secs(interval_secs));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match mark_overdue_ticker(&ctx).await {
                Ok(out) => {
                    if !out.overdue_invoices.is_empty() {
                        tracing::info!(
                            count = out.overdue_invoices.len(),
                            "overdue invoices flagged"
                        );
                    }
                }
                Err(e) => tracing::error!(error = %e, "overdue tick failed"),
            }
        }
    })
}

async fn wait_shutdown() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sigint = signal(SignalKind::interrupt())?;
        tokio::select! {
            _ = sigterm.recv() => Ok(()),
            _ = sigint.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        signal::ctrl_c().await?;
        Ok(())
    }
}
