//! Credit-note tools.
//!
//! - `inv_creditnote_draft` → [`inv_commands::create_credit_note`]
//! - `inv_creditnote_issue` → [`inv_commands::issue_credit_note`]

use rust_decimal::Decimal;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use inv_commands::{
    create_credit_note, issue_credit_note, CoreCtx, CreateCreditNoteInput, IssueCreditNoteInput,
};
use inv_core::domain::ids::{CreditNoteId, InvoiceId};

use crate::error::McpError;
use crate::tools::common::{mcp_actor, mcp_channel};

/// Input for `inv_creditnote_draft`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreditNoteDraftInput {
    /// Invoice the credit note credits.
    pub invoice_id: String,
    /// Credit amount (> 0). Decimal-string.
    #[schemars(with = "String")]
    pub amount: Decimal,
    /// Optional free-form reason.
    #[serde(default)]
    pub reason: Option<String>,
    /// If auto-created from a refund event, the inbound bus event id.
    #[serde(default)]
    pub refund_ref: Option<String>,
    /// Optional caller-supplied dedupe key.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// Run `create_credit_note`.
pub async fn draft(
    ctx: &CoreCtx,
    input: CreditNoteDraftInput,
) -> Result<serde_json::Value, McpError> {
    let invoice_id: InvoiceId =
        input
            .invoice_id
            .parse()
            .map_err(|e: inv_core::domain::ids::IdError| {
                McpError::Decode(format!("invoice_id: {e}"))
            })?;
    let req = CreateCreditNoteInput {
        invoice_id,
        amount: input.amount,
        reason: input.reason,
        refund_ref: input.refund_ref,
        idempotency_key: input.idempotency_key,
        actor: mcp_actor(),
        channel: mcp_channel(),
    };
    let out = create_credit_note(ctx, req).await?;
    crate::tools::common::to_value(&out.credit_note)
}

/// Input for `inv_creditnote_issue`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreditNoteIssueInput {
    /// Credit-note id.
    pub credit_note_id: String,
    /// Optional caller-supplied dedupe key.
    #[serde(default)]
    pub idempotency_key: Option<String>,
}

/// Run `issue_credit_note`.
pub async fn issue(
    ctx: &CoreCtx,
    input: CreditNoteIssueInput,
) -> Result<serde_json::Value, McpError> {
    let credit_note_id: CreditNoteId =
        input
            .credit_note_id
            .parse()
            .map_err(|e: inv_core::domain::ids::IdError| {
                McpError::Decode(format!("credit_note_id: {e}"))
            })?;
    let req = IssueCreditNoteInput {
        credit_note_id,
        idempotency_key: input.idempotency_key,
        actor: mcp_actor(),
        channel: mcp_channel(),
    };
    let out = issue_credit_note(ctx, req).await?;
    crate::tools::common::to_value(&out.credit_note)
}
