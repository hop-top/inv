//! Read-only MCP resources exposed by `inv-mcp`.
//!
//! Every readable entity in the store gets an MCP-resource URI:
//!
//! - `inv://invoice/<id>`     → `InvoiceRepo::get`
//! - `inv://creditnote/<id>`  → `CreditNoteRepo::get`
//! - `inv://schedule/<id>`    → `ScheduleRepo::get`
//! - `inv://reminder/<id>`    → `ReminderRepo::get`
//! - `inv://customer/<id>`    → `CustomerRepo::get`
//!
//! Reading any of these returns a JSON document (`mime_type:
//! "application/json"`) — the full serialized domain struct.

use std::sync::Arc;

use rmcp::model::{
    RawResource, ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents,
};
use rmcp::model::AnnotateAble;

use inv_core::domain::ids::{CreditNoteId, CustomerId, InvoiceId, ReminderId, ScheduleId};
use inv_commands::CoreCtx;
use inv_store::repo::credit_note::CreditNoteRepo;
use inv_store::repo::customer::CustomerRepo;
use inv_store::repo::invoice::InvoiceRepo;
use inv_store::repo::reminder::ReminderRepo;
use inv_store::repo::schedule::ScheduleRepo;

use crate::error::McpError;

/// URI scheme used by all `inv-mcp` resources.
pub const URI_SCHEME: &str = "inv";

/// MIME type for every resource body — they're all JSON documents.
const MIME_JSON: &str = "application/json";

/// Static template list returned from `resources/templates/list`.
///
/// rmcp's `RawResource` doubles as both "concrete instance" and
/// "template" because the URIs we list here are templates — clients
/// substitute the `<id>` placeholder before calling `resources/read`.
pub fn template_list() -> Vec<Resource> {
    [
        ("inv://invoice/<id>", "inv:invoice"),
        ("inv://creditnote/<id>", "inv:creditnote"),
        ("inv://schedule/<id>", "inv:schedule"),
        ("inv://reminder/<id>", "inv:reminder"),
        ("inv://customer/<id>", "inv:customer"),
    ]
    .into_iter()
    .map(|(uri, name)| {
        RawResource::new(uri, name)
            .with_mime_type(MIME_JSON)
            .with_description(
                "JSON view of the entity. Replace <id> with the entity's MTI before reading.",
            )
            .no_annotation()
    })
    .collect()
}

/// Resolve `inv://<kind>/<id>` to a `ReadResourceResult`.
pub async fn read(
    ctx: &Arc<CoreCtx>,
    request: ReadResourceRequestParams,
) -> Result<ReadResourceResult, McpError> {
    let (kind, id_str) = parse_uri(&request.uri)?;
    let body = match kind {
        "invoice" => {
            let id: InvoiceId = id_str
                .parse()
                .map_err(|e| McpError::InvalidUri(format!("bad invoice id: {e}")))?;
            let inv = InvoiceRepo::new(&ctx.db)
                .get(&id)
                .await?
                .ok_or_else(|| McpError::NotFound(format!("invoice {id}")))?;
            serde_json::to_string_pretty(&inv).map_err(|e| McpError::Decode(e.to_string()))?
        }
        "creditnote" => {
            let id: CreditNoteId = id_str
                .parse()
                .map_err(|e| McpError::InvalidUri(format!("bad credit-note id: {e}")))?;
            let cn = CreditNoteRepo::new(&ctx.db)
                .get(&id)
                .await?
                .ok_or_else(|| McpError::NotFound(format!("credit note {id}")))?;
            serde_json::to_string_pretty(&cn).map_err(|e| McpError::Decode(e.to_string()))?
        }
        "schedule" => {
            let id: ScheduleId = id_str
                .parse()
                .map_err(|e| McpError::InvalidUri(format!("bad schedule id: {e}")))?;
            let sched = ScheduleRepo::new(&ctx.db)
                .get(&id)
                .await?
                .ok_or_else(|| McpError::NotFound(format!("schedule {id}")))?;
            serde_json::to_string_pretty(&sched).map_err(|e| McpError::Decode(e.to_string()))?
        }
        "reminder" => {
            let id: ReminderId = id_str
                .parse()
                .map_err(|e| McpError::InvalidUri(format!("bad reminder id: {e}")))?;
            let rem = ReminderRepo::new(&ctx.db)
                .get(&id)
                .await?
                .ok_or_else(|| McpError::NotFound(format!("reminder {id}")))?;
            serde_json::to_string_pretty(&rem).map_err(|e| McpError::Decode(e.to_string()))?
        }
        "customer" => {
            let id: CustomerId = id_str
                .parse()
                .map_err(|e| McpError::InvalidUri(format!("bad customer id: {e}")))?;
            let c = CustomerRepo::new(&ctx.db)
                .get(&id)
                .await?
                .ok_or_else(|| McpError::NotFound(format!("customer {id}")))?;
            serde_json::to_string_pretty(&c).map_err(|e| McpError::Decode(e.to_string()))?
        }
        other => {
            return Err(McpError::InvalidUri(format!(
                "unknown inv resource kind `{other}` (expected invoice|creditnote|schedule|reminder|customer)"
            )))
        }
    };

    Ok(ReadResourceResult::new(vec![
        ResourceContents::text(body, request.uri).with_mime_type(MIME_JSON),
    ]))
}

/// Parse `inv://<kind>/<id>` into `(kind, id)`. Bare `inv://kind/id` is
/// the only form accepted; query strings, fragments, and additional
/// path segments are rejected.
fn parse_uri(uri: &str) -> Result<(&str, &str), McpError> {
    let rest = uri
        .strip_prefix(URI_SCHEME)
        .and_then(|s| s.strip_prefix("://"))
        .ok_or_else(|| {
            McpError::InvalidUri(format!(
                "expected `inv://<kind>/<id>`, got `{uri}`"
            ))
        })?;
    let mut parts = rest.splitn(2, '/');
    let kind = parts
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| McpError::InvalidUri(format!("missing kind in `{uri}`")))?;
    let id = parts
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| McpError::InvalidUri(format!("missing id in `{uri}`")))?;
    if id.contains('/') {
        return Err(McpError::InvalidUri(format!(
            "extra path components in `{uri}`"
        )));
    }
    Ok((kind, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_well_formed_uri() {
        let (kind, id) = parse_uri("inv://invoice/inv_abc123").unwrap();
        assert_eq!(kind, "invoice");
        assert_eq!(id, "inv_abc123");
    }

    #[test]
    fn rejects_wrong_scheme() {
        assert!(parse_uri("file://x").is_err());
    }

    #[test]
    fn rejects_missing_id() {
        assert!(parse_uri("inv://invoice/").is_err());
        assert!(parse_uri("inv://invoice").is_err());
    }

    #[test]
    fn rejects_extra_path() {
        assert!(parse_uri("inv://invoice/x/y").is_err());
    }

    #[test]
    fn template_list_has_five_entries() {
        let templates = template_list();
        assert_eq!(templates.len(), 5);
    }
}
