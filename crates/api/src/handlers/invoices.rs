//! Invoice routes — draft / issue / send / pay / void + read.
//!
//! Plus three internal-only ticker routes (`/v1/tick/*`) used by
//! `inv-server` and the read-only tax-table dump.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use inv_commands::{
    draft_invoice, issue_invoice, mark_overdue_ticker, mark_paid, reminders_tick,
    schedules_tick, send_invoice, void_invoice, Actor, Channel, DraftInvoiceInput,
    DraftLineInput, IssueInvoiceInput, MarkPaidInput, SendInvoiceInput, VoidInvoiceInput,
};
use inv_core::domain::ids::{CustomerId, InvoiceId};
use inv_core::domain::invoice::{Invoice, InvoiceLine, InvoiceState, TaxCategory};
use inv_core::domain::jurisdiction::Jurisdiction;
use inv_core::domain::money::Currency;
use inv_store::repo::invoice::{InvoiceFilter, InvoiceLineRepo, InvoiceRepo};

use crate::error::ApiError;
use crate::state::ApiState;
use crate::webhook;

// =============================================================================
// shared JSON shapes
// =============================================================================

/// Wire body for `POST /v1/invoices`.
#[derive(Debug, Deserialize)]
pub struct DraftBody {
    /// Customer to bill.
    pub customer_id: String,
    /// Seller jurisdiction ISO 3166-2 code (`CA-QC`, `US-DE`, `DZ-16`).
    pub seller_jurisdiction: String,
    /// Invoice currency (USD/CAD/DZD at v1).
    pub currency: String,
    /// Line items.
    pub lines: Vec<LineBody>,
    /// Caller-supplied dedupe key.
    #[serde(default)]
    pub idempotency_key: Option<String>,
    /// Optional payment due date (RFC 3339).
    #[serde(default)]
    pub due_at: Option<DateTime<Utc>>,
    /// Per-invoice template override (filesystem path).
    #[serde(default)]
    pub template_path: Option<String>,
}

/// One line in [`DraftBody`].
#[derive(Debug, Deserialize)]
pub struct LineBody {
    /// Description.
    pub description: String,
    /// Quantity.
    pub quantity: Decimal,
    /// Per-unit price.
    pub unit_price: Decimal,
    /// Tax category. Defaults to `standard`.
    #[serde(default)]
    pub tax_category: TaxCategory,
}

/// Wire shape for a draft / issued / paid invoice response.
#[derive(Debug, Serialize)]
pub struct InvoiceResponse {
    /// The invoice record.
    pub invoice: Invoice,
    /// Line items (sorted by position).
    pub lines: Vec<InvoiceLine>,
    /// Bus events the command would emit (T-0014 wires the real bus).
    pub emitted_events: Vec<EmittedEventOut>,
    /// True if the command short-circuited on an idempotency hit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_replay: Option<bool>,
}

/// Wire shape for [`inv_commands::EmittedEvent`].
#[derive(Debug, Serialize)]
pub struct EmittedEventOut {
    /// Dotted topic name.
    pub topic: String,
    /// Event payload.
    pub payload: Value,
    /// Wall-clock at emit time.
    pub emitted_at: DateTime<Utc>,
}

impl From<inv_commands::EmittedEvent> for EmittedEventOut {
    fn from(e: inv_commands::EmittedEvent) -> Self {
        Self {
            topic: e.topic,
            payload: e.payload,
            emitted_at: e.emitted_at,
        }
    }
}

fn into_events(events: Vec<inv_commands::EmittedEvent>) -> Vec<EmittedEventOut> {
    events.into_iter().map(Into::into).collect()
}

// =============================================================================
// helpers
// =============================================================================

fn parse_invoice_id(s: &str) -> Result<InvoiceId, ApiError> {
    s.parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid invoice id `{s}`: {e}")))
}

fn parse_customer_id(s: &str) -> Result<CustomerId, ApiError> {
    s.parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid customer id `{s}`: {e}")))
}

fn parse_currency(s: &str) -> Result<Currency, ApiError> {
    s.parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid currency `{s}`: {e}")))
}

fn parse_jurisdiction(s: &str) -> Result<Jurisdiction, ApiError> {
    s.parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid jurisdiction `{s}`: {e}")))
}

fn parse_state(s: &str) -> Result<InvoiceState, ApiError> {
    Ok(match s {
        "draft" => InvoiceState::Draft,
        "issued" => InvoiceState::Issued,
        "sent" => InvoiceState::Sent,
        "viewed" => InvoiceState::Viewed,
        "partially_paid" => InvoiceState::PartiallyPaid,
        "paid" => InvoiceState::Paid,
        "voided" => InvoiceState::Voided,
        other => return Err(ApiError::BadRequest(format!("invalid invoice state `{other}`"))),
    })
}

/// Default Actor for any HTTP-originated request. The bearer-token
/// principal is opaque to the command layer at v1 — T-0029 will replace
/// `"http"` with the resolved identity.
fn api_actor() -> Actor {
    Actor::Api { name: "http".into() }
}

// =============================================================================
// POST /v1/invoices  -> draft
// =============================================================================

/// `POST /v1/invoices` — create a draft invoice.
pub async fn draft(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<DraftBody>,
) -> Result<(StatusCode, Json<InvoiceResponse>), ApiError> {
    let customer_id = parse_customer_id(&body.customer_id)?;
    let seller_jurisdiction = parse_jurisdiction(&body.seller_jurisdiction)?;
    let currency = parse_currency(&body.currency)?;

    let lines = body
        .lines
        .into_iter()
        .map(|l| DraftLineInput {
            description: l.description,
            quantity: l.quantity,
            unit_price: l.unit_price,
            tax_category: l.tax_category,
        })
        .collect();

    let input = DraftInvoiceInput {
        customer_id,
        seller_jurisdiction,
        currency,
        lines,
        idempotency_key: body.idempotency_key,
        actor: api_actor(),
        channel: Channel::Api,
        due_at: body.due_at,
        template_path: body.template_path,
    };

    let out = draft_invoice(&state.ctx, input).await?;
    let status = if out.idempotency_replay {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((
        status,
        Json(InvoiceResponse {
            invoice: out.invoice,
            lines: out.lines,
            emitted_events: into_events(out.emitted_events),
            idempotency_replay: Some(out.idempotency_replay),
        }),
    ))
}

// =============================================================================
// GET /v1/invoices  -> list  (?customer=&state=&limit=&offset=)
// =============================================================================

/// Query parameters for [`list`].
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    /// Restrict to a specific customer.
    #[serde(default)]
    pub customer: Option<String>,
    /// Restrict to a specific state.
    #[serde(default)]
    pub state: Option<String>,
    /// Max rows (default 100).
    #[serde(default)]
    pub limit: Option<i64>,
    /// Offset for paging.
    #[serde(default)]
    pub offset: Option<i64>,
}

/// `GET /v1/invoices` — list invoices.
pub async fn list(
    State(state): State<Arc<ApiState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<Invoice>>, ApiError> {
    let customer_id = match q.customer.as_deref() {
        None | Some("") => None,
        Some(s) => Some(parse_customer_id(s)?),
    };
    let inv_state = match q.state.as_deref() {
        None | Some("") => None,
        Some(s) => Some(parse_state(s)?),
    };
    let filter = InvoiceFilter {
        customer_id,
        state: inv_state,
        limit: q.limit,
        offset: q.offset,
    };
    let rows = InvoiceRepo::new(&state.ctx.db)
        .list(&filter)
        .await
        .map_err(inv_commands::CoreError::from)?;
    Ok(Json(rows))
}

// =============================================================================
// GET /v1/invoices/:id
// =============================================================================

/// `GET /v1/invoices/{id}` — fetch a single invoice + lines.
pub async fn get(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_invoice_id(&id)?;
    let repo = InvoiceRepo::new(&state.ctx.db);
    let invoice = repo
        .get(&id)
        .await
        .map_err(inv_commands::CoreError::from)?
        .ok_or_else(|| ApiError::NotFound(format!("invoice {id}")))?;
    let lines = InvoiceLineRepo::new(&state.ctx.db)
        .list_for_invoice(&id)
        .await
        .map_err(inv_commands::CoreError::from)?;
    Ok(Json(json!({ "invoice": invoice, "lines": lines })))
}

// =============================================================================
// POST /v1/invoices/:id/issue
// =============================================================================

/// Optional body for [`issue`].
#[derive(Debug, Default, Deserialize)]
pub struct IssueBody {
    /// Caller-supplied dedupe key (forwarded to the command).
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// `POST /v1/invoices/{id}/issue`.
///
/// The request body is optional; an empty `{}` or no body at all both
/// trigger the default-everything path. We parse the body manually so
/// callers don't have to supply a Content-Type when there's nothing to
/// say.
pub async fn issue(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Result<Json<InvoiceResponse>, ApiError> {
    let id = parse_invoice_id(&id)?;
    let body: IssueBody = parse_optional_body(&bytes)?;
    let input = IssueInvoiceInput {
        invoice_id: id,
        idempotency_key: body.idempotency_key,
        actor: api_actor(),
        channel: Channel::Api,
    };
    let out = issue_invoice(&state.ctx, input).await?;
    Ok(Json(InvoiceResponse {
        invoice: out.invoice,
        lines: out.lines,
        emitted_events: into_events(out.emitted_events),
        idempotency_replay: None,
    }))
}

/// Parse an optional JSON body. An empty body returns the type's
/// `Default` value.
fn parse_optional_body<T: serde::de::DeserializeOwned + Default>(
    bytes: &axum::body::Bytes,
) -> Result<T, ApiError> {
    if bytes.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_slice(bytes)
        .map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))
}

// =============================================================================
// POST /v1/invoices/:id/send
// =============================================================================

/// Body for [`send`].
#[derive(Debug, Deserialize)]
pub struct SendBody {
    /// Destination URI. `file://`, `stdout`, `webhook://`, `link://`.
    pub destination_uri: String,
    /// Caller-supplied dedupe key.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// `POST /v1/invoices/{id}/send`.
///
/// The adapter handles two extra schemes that the command layer leaves
/// to channels:
///
/// - `webhook://...` — POST the rendered bytes with `X-Inv-Signature`.
/// - `link://` — mint a signed `/v/{token}` URL and return it.
pub async fn send(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(body): Json<SendBody>,
) -> Result<Json<Value>, ApiError> {
    let invoice_id = parse_invoice_id(&id)?;

    // link:// — mint a signed token + return the URL. No state mutation;
    // the actual `inv.billing.invoice.viewed` event fires when someone
    // fetches the URL.
    if body.destination_uri == "link://" || body.destination_uri.starts_with("link://") {
        // Confirm the invoice exists before minting a token; otherwise
        // we'd hand out unbounded tokens against bogus ids.
        let repo = InvoiceRepo::new(&state.ctx.db);
        let _ = repo
            .get(&invoice_id)
            .await
            .map_err(inv_commands::CoreError::from)?
            .ok_or_else(|| ApiError::NotFound(format!("invoice {invoice_id}")))?;
        let ttl = state.config.link_ttl.as_secs();
        let token = crate::signed_link::sign(
            &invoice_id.to_string(),
            ttl,
            &state.config.link_signing_key,
        );
        let url = format!(
            "{}/v/{}",
            state.config.public_base_url.trim_end_matches('/'),
            token
        );
        return Ok(Json(json!({
            "url": url,
            "token": token,
            "expires_in_seconds": ttl,
        })));
    }

    // webhook:// — render via send_invoice into an in-memory sink, then
    // POST the bytes with X-Inv-Signature. The command itself doesn't
    // know about webhook semantics; we drive it through `stdout` against
    // a Vec<u8> sink to capture the rendered bytes, then handle the FSM
    // mutation + dispatch ourselves.
    if body.destination_uri.starts_with("webhook://")
        || body.destination_uri.starts_with("webhook+http://")
    {
        return send_webhook(&state, invoice_id, body).await;
    }

    // file:// / stdout — forward to the command. The CLI uses stdout
    // with a real stdout sink; the API has no equivalent, so we just
    // accept file:// and reject stdout with a clear error.
    if body.destination_uri == "stdout" {
        return Err(ApiError::BadRequest(
            "send: `stdout` destination is not supported over HTTP — use webhook:// or file:// or link://".into(),
        ));
    }
    let input = SendInvoiceInput {
        invoice_id,
        destination_uri: body.destination_uri.clone(),
        idempotency_key: body.idempotency_key.clone(),
        actor: api_actor(),
        channel: Channel::Api,
        sink: None,
    };
    let out = send_invoice(&state.ctx, input).await?;
    Ok(Json(json!({
        "invoice": out.invoice,
        "delivered_to": out.delivered_to,
        "emitted_events": into_events(out.emitted_events),
    })))
}

async fn send_webhook(
    state: &Arc<ApiState>,
    invoice_id: InvoiceId,
    body: SendBody,
) -> Result<Json<Value>, ApiError> {
    // Capture bytes by driving send_invoice through a Vec<u8> sink under
    // the `stdout` scheme. Translating the destination URI here keeps
    // the command FSM happy (it would otherwise reject `webhook://` as
    // NotImplemented per design §7) while preserving its history-row
    // semantics — albeit with `destination = "stdout"`. Once T-0014 lets
    // the command emit raw bytes without an FSM mutation, we'll thread
    // the real `webhook://` destination through.
    let mut sink: Vec<u8> = Vec::new();
    let input = SendInvoiceInput {
        invoice_id,
        destination_uri: "stdout".into(),
        idempotency_key: body.idempotency_key.clone(),
        actor: api_actor(),
        channel: Channel::Api,
        sink: Some(&mut sink),
    };
    let out = send_invoice(&state.ctx, input).await?;

    // Dispatch over HTTP with X-Inv-Signature.
    webhook::dispatch(
        &state.http,
        &body.destination_uri,
        "application/pdf",
        out.pdf.clone(),
        &state.config.webhook_signing_key,
    )
    .await?;

    Ok(Json(json!({
        "invoice": out.invoice,
        "delivered_to": body.destination_uri,
        "emitted_events": into_events(out.emitted_events),
    })))
}

// =============================================================================
// POST /v1/invoices/:id/pay
// =============================================================================

/// Body for [`pay`].
#[derive(Debug, Deserialize)]
pub struct PayBody {
    /// Payment amount (positive).
    pub amount: Decimal,
    /// When the payment was received (RFC 3339). Defaults to `now`.
    #[serde(default)]
    pub received_at: Option<DateTime<Utc>>,
    /// Inbound bus event id (when triggered by a bus consumer
    /// upstream — included for parity with the command).
    #[serde(default)]
    pub bus_event_id: Option<String>,
    /// Caller-supplied dedupe key.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// `POST /v1/invoices/{id}/pay`.
pub async fn pay(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(body): Json<PayBody>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_invoice_id(&id)?;
    let input = MarkPaidInput {
        invoice_id: id,
        amount: body.amount,
        received_at: body.received_at,
        idempotency_key: body.idempotency_key,
        bus_event_id: body.bus_event_id,
        actor: api_actor(),
        channel: Channel::Api,
    };
    let out = mark_paid(&state.ctx, input).await?;
    Ok(Json(json!({
        "invoice": out.invoice,
        "fully_paid": out.fully_paid,
        "emitted_events": into_events(out.emitted_events),
    })))
}

// =============================================================================
// POST /v1/invoices/:id/void
// =============================================================================

/// Body for [`void`].
#[derive(Debug, Default, Deserialize)]
pub struct VoidBody {
    /// Free-form reason recorded on the history row.
    #[serde(default)]
    pub reason: Option<String>,
    /// Caller-supplied dedupe key.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// `POST /v1/invoices/{id}/void`.
pub async fn void(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    bytes: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    let id = parse_invoice_id(&id)?;
    let body: VoidBody = parse_optional_body(&bytes)?;
    let input = VoidInvoiceInput {
        invoice_id: id,
        reason: body.reason,
        idempotency_key: body.idempotency_key,
        actor: api_actor(),
        channel: Channel::Api,
    };
    let out = void_invoice(&state.ctx, input).await?;
    Ok(Json(json!({
        "invoice": out.invoice,
        "emitted_events": into_events(out.emitted_events),
    })))
}

// =============================================================================
// POST /v1/tick/schedules | /v1/tick/reminders | /v1/tick/overdue
// =============================================================================

/// `POST /v1/tick/schedules` — materialise due schedules.
pub async fn tick_schedules(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Value>, ApiError> {
    let out = schedules_tick(&state.ctx).await?;
    Ok(Json(json!({
        "ran_schedule_ids": out.ran_schedule_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        "drafts_count": out.drafts.len(),
        "emitted_events": into_events(out.emitted_events),
    })))
}

/// `POST /v1/tick/reminders` — dispatch due reminders.
pub async fn tick_reminders(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Value>, ApiError> {
    let out = reminders_tick(&state.ctx).await?;
    Ok(Json(json!({
        "sent_reminders": out.sent_reminders,
        "emitted_events": into_events(out.emitted_events),
    })))
}

/// `POST /v1/tick/overdue` — emit overdue events for unpaid invoices.
pub async fn tick_overdue(
    State(state): State<Arc<ApiState>>,
) -> Result<Json<Value>, ApiError> {
    let out = mark_overdue_ticker(&state.ctx).await?;
    Ok(Json(json!({
        "overdue_invoices": out.overdue_invoices,
        "emitted_events": into_events(out.emitted_events),
    })))
}

// =============================================================================
// GET /v1/tax/rates
// =============================================================================

/// `GET /v1/tax/rates` — dump the in-memory tax table.
///
/// Read-only debug helper; useful for operator verification when
/// commissioning a new rate file.
pub async fn list_tax_rates(State(state): State<Arc<ApiState>>) -> Json<Value> {
    Json(json!({
        "rates": state.ctx.tax_table.rates,
    }))
}

