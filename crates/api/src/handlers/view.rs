//! Public signed-link view route — `GET /v/{token}`.
//!
//! Per design §7, fetching the URL renders the invoice and emits
//! `inv.billing.invoice.viewed`. State transitions Sent → Viewed (the
//! FSM allows it; multiple views are idempotent). The token is verified
//! before we touch the store so a 404 leaks nothing.
//!
//! View tracking + bus emission is best-effort on the publisher side:
//! when the [`hop_top_inv_commands::CoreCtx`] doesn't carry a bus publisher we
//! still update the row in the store (the history row doubles as the
//! outbox and the relay will pick it up next tick). The adapter does
//! NOT depend on the publisher to serve content.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::IntoResponse;
use chrono::Utc;

use hop_top_inv_bus::{InvoiceViewed, TOPIC_INVOICE_VIEWED};
use hop_top_inv_core::domain::ids::{HistoryId, InvoiceId};
use hop_top_inv_core::domain::invoice::{HistoryChannel, InvoiceState, InvoiceStateHistory};
use hop_top_inv_core::render::{render_html, RenderContext};
use hop_top_inv_store::repo::customer::CustomerRepo;
use hop_top_inv_store::repo::history::InvoiceHistoryRepo;
use hop_top_inv_store::repo::invoice::{InvoiceLineRepo, InvoiceRepo};

use crate::error::ApiError;
use crate::signed_link;
use crate::state::ApiState;

/// `GET /v/{token}` — public, no auth.
///
/// Verifies the signed token, fetches the invoice + lines + customer,
/// renders HTML, and (if the invoice is in `Sent`) advances it to
/// `Viewed` + appends a history row marked `viewed`. Subsequent views
/// re-render but do not re-emit the FSM transition.
pub async fn view_invoice(
    State(state): State<Arc<ApiState>>,
    Path(token): Path<String>,
) -> Result<axum::response::Response, ApiError> {
    let verified = signed_link::verify(&token, &state.config.link_signing_key)
        .map_err(|_| ApiError::InvalidLink)?;
    let invoice_id: InvoiceId = verified
        .invoice_id
        .parse()
        .map_err(|_| ApiError::InvalidLink)?;

    // Fetch invoice + lines + customer. Any miss collapses to InvalidLink
    // (404) so we never confirm "this token decoded for an invoice that
    // doesn't exist".
    let inv_repo = InvoiceRepo::new(&state.ctx.db);
    let mut invoice = inv_repo
        .get(&invoice_id)
        .await
        .map_err(hop_top_inv_commands::CoreError::from)?
        .ok_or(ApiError::InvalidLink)?;
    let lines = InvoiceLineRepo::new(&state.ctx.db)
        .list_for_invoice(&invoice_id)
        .await
        .map_err(hop_top_inv_commands::CoreError::from)?;
    let customer = CustomerRepo::new(&state.ctx.db)
        .get(&invoice.customer_id)
        .await
        .map_err(hop_top_inv_commands::CoreError::from)?
        .ok_or(ApiError::InvalidLink)?;

    // Render HTML. Templates that hold `null` for tax fields still work
    // because `RenderContext::new` doesn't touch tax until it's set.
    let render_ctx = RenderContext::new(invoice.clone(), customer, lines);
    let template_path = invoice.template_path.as_deref().map(std::path::Path::new);
    let html = render_html(template_path, &render_ctx)
        .await
        .map_err(hop_top_inv_commands::CoreError::from)?;

    // Advance Sent → Viewed if we're in Sent. Multiple views are
    // idempotent — once we're in Viewed we just re-render.
    if matches!(invoice.state, InvoiceState::Sent) {
        let now = state.ctx.clock.now();
        let from_state = invoice.state;
        invoice.state = InvoiceState::Viewed;
        invoice.viewed_at = Some(now);
        invoice.updated_at = now;

        let history = InvoiceStateHistory {
            id: HistoryId::new(),
            invoice_id: invoice.id.clone(),
            from_state: Some(from_state),
            to_state: InvoiceState::Viewed,
            event: "view".into(),
            actor: Some("link.viewer".into()),
            channel: HistoryChannel::Api,
            bus_event_id: None,
            reason: None,
            occurred_at: now,
            published_at: None,
            metadata: BTreeMap::new(),
        };

        let mut tx = state.ctx.db.begin().await.map_err(|e| {
            hop_top_inv_commands::CoreError::Repo(hop_top_inv_store::StoreError::from(e))
        })?;
        InvoiceRepo::save_in_tx(&mut tx, &invoice)
            .await
            .map_err(hop_top_inv_commands::CoreError::from)?;
        InvoiceHistoryRepo::save_in_tx(&mut tx, &history)
            .await
            .map_err(hop_top_inv_commands::CoreError::from)?;
        tx.commit().await.map_err(|e| {
            hop_top_inv_commands::CoreError::Repo(hop_top_inv_store::StoreError::from(e))
        })?;

        // Emit `inv.billing.invoice.viewed`. The history row is the
        // canonical outbox entry (the relay will publish it on its next
        // tick); when `ctx.publisher` is wired (T-0031) we ALSO publish
        // synchronously so subscribers — notably the WebSocket
        // BroadcastPublisher — see the view in real time. Publish failure
        // is logged, not propagated: the viewer still gets HTML and the
        // outbox row still exists for retry.
        if let Some(publisher) = state.ctx.publisher.as_ref() {
            let payload = InvoiceViewed {
                invoice_id: invoice.id.to_string(),
                actor: "link.viewer".into(),
                channel: "api".into(),
            };
            match serde_json::to_value(&payload) {
                Ok(value) => {
                    if let Err(e) = publisher.publish(TOPIC_INVOICE_VIEWED, value, now).await {
                        tracing::warn!(
                            error = %e,
                            invoice_id = %invoice.id,
                            "synchronous viewed publish failed; outbox relay will retry"
                        );
                    }
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    invoice_id = %invoice.id,
                    "failed to serialise InvoiceViewed payload"
                ),
            }
        }
    }

    // Build a text/html response with the rendered body.
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    // Cache: never. The signed link is per-recipient; intermediaries
    // shouldn't cache it.
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));

    Ok((axum::http::StatusCode::OK, headers, html).into_response())
}

// Silence Utc-import warning if a refactor removes the direct call.
const _: Option<chrono::DateTime<Utc>> = None;
