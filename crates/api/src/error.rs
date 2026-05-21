//! RFC 9457 problem-detail responses.
//!
//! Every fallible handler returns `Result<T, ApiError>`. [`ApiError`]
//! implements [`axum::response::IntoResponse`] and serialises to a
//! `application/problem+json` body shaped as
//!
//! ```json
//! {
//!   "type":    "https://errors.inv.hop.top/<slug>",
//!   "title":   "Short, human-readable summary",
//!   "status":  400,
//!   "detail":  "Long-form detail",
//!   "instance": "/v1/invoices/invoice_01..."
//! }
//! ```
//!
//! Mapping from [`inv_commands::CoreError`] follows the task spec:
//!
//! | CoreError variant     | HTTP status |
//! |-----------------------|------------:|
//! | `Validation`          | 400         |
//! | `Idempotency`         | 409         |
//! | `FsmTransition`       | 409         |
//! | `Repo`                | 500         |
//! | `Tax`                 | 500         |
//! | `Render`              | 500         |
//! | `Pdf`                 | 500         |
//! | `NotFound`            | 404         |
//! | `NotImplemented`      | 501         |

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use inv_commands::CoreError;

/// All errors surfaced by api handlers.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// Forwarded from a command call.
    #[error(transparent)]
    Core(#[from] CoreError),

    /// Request body / query parameters failed to parse.
    #[error("bad request: {0}")]
    BadRequest(String),

    /// Authorisation failed (missing or unrecognised bearer token).
    #[error("unauthorized")]
    Unauthorized,

    /// Caller-asked-for resource does not exist (no command was even
    /// invoked — e.g. a malformed ID or a 404 from the store layer
    /// before reaching the command).
    #[error("not found: {0}")]
    NotFound(String),

    /// Signed-link verification failed: token expired, signature
    /// mismatch, or malformed. We collapse the three cases into one 404
    /// so attackers can't probe for valid invoice IDs through the
    /// public route.
    #[error("invalid or expired link")]
    InvalidLink,

    /// Webhook delivery failed (the receiver did not respond or
    /// responded with a non-2xx status). Returned as a 502 because the
    /// command layer succeeded but the channel adapter could not
    /// complete the side effect.
    #[error("webhook dispatch failed: {0}")]
    WebhookDispatch(String),

    /// Catch-all internal error. Use sparingly; prefer a typed variant.
    #[error("internal: {0}")]
    Internal(String),
}

/// Wire shape — RFC 9457 problem-detail.
#[derive(Debug, Serialize)]
struct ProblemDetail<'a> {
    #[serde(rename = "type")]
    typ: String,
    title: &'a str,
    status: u16,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instance: Option<String>,
}

impl ApiError {
    fn status(&self) -> StatusCode {
        match self {
            ApiError::Core(err) => core_status(err),
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::NotFound(_) | ApiError::InvalidLink => StatusCode::NOT_FOUND,
            ApiError::WebhookDispatch(_) => StatusCode::BAD_GATEWAY,
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn slug(&self) -> &'static str {
        match self {
            ApiError::Core(err) => core_slug(err),
            ApiError::BadRequest(_) => "bad-request",
            ApiError::Unauthorized => "unauthorized",
            ApiError::NotFound(_) => "not-found",
            ApiError::InvalidLink => "invalid-link",
            ApiError::WebhookDispatch(_) => "webhook-dispatch-failed",
            ApiError::Internal(_) => "internal",
        }
    }

    fn title(&self) -> &'static str {
        match self {
            ApiError::Core(err) => core_title(err),
            ApiError::BadRequest(_) => "Bad request",
            ApiError::Unauthorized => "Unauthorized",
            ApiError::NotFound(_) => "Not found",
            ApiError::InvalidLink => "Invalid or expired link",
            ApiError::WebhookDispatch(_) => "Webhook dispatch failed",
            ApiError::Internal(_) => "Internal server error",
        }
    }

    fn detail(&self) -> String {
        match self {
            ApiError::Core(err) => err.to_string(),
            ApiError::BadRequest(s)
            | ApiError::NotFound(s)
            | ApiError::WebhookDispatch(s)
            | ApiError::Internal(s) => s.clone(),
            ApiError::Unauthorized => {
                "Authorization header missing or unrecognised bearer token".into()
            }
            ApiError::InvalidLink => "Token signature mismatch, expired, or malformed".into(),
        }
    }
}

fn core_status(err: &CoreError) -> StatusCode {
    match err {
        CoreError::Validation(_) => StatusCode::BAD_REQUEST,
        CoreError::Idempotency(_) => StatusCode::CONFLICT,
        CoreError::FsmTransition(_) => StatusCode::CONFLICT,
        CoreError::NotFound(_) => StatusCode::NOT_FOUND,
        CoreError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
        CoreError::Repo(_)
        | CoreError::Tax(_)
        | CoreError::Render(_)
        | CoreError::Pdf(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn core_slug(err: &CoreError) -> &'static str {
    match err {
        CoreError::Validation(_) => "validation",
        CoreError::Idempotency(_) => "idempotency-conflict",
        CoreError::FsmTransition(_) => "fsm-transition",
        CoreError::Repo(_) => "repo",
        CoreError::Tax(_) => "tax",
        CoreError::Render(_) => "render",
        CoreError::Pdf(_) => "pdf",
        CoreError::NotFound(_) => "not-found",
        CoreError::NotImplemented(_) => "not-implemented",
    }
}

fn core_title(err: &CoreError) -> &'static str {
    match err {
        CoreError::Validation(_) => "Validation error",
        CoreError::Idempotency(_) => "Idempotency conflict",
        CoreError::FsmTransition(_) => "FSM transition rejected",
        CoreError::Repo(_) => "Repository error",
        CoreError::Tax(_) => "Tax engine error",
        CoreError::Render(_) => "Render error",
        CoreError::Pdf(_) => "PDF render error",
        CoreError::NotFound(_) => "Not found",
        CoreError::NotImplemented(_) => "Not implemented",
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = ProblemDetail {
            typ: format!("https://errors.inv.hop.top/{}", self.slug()),
            title: self.title(),
            status: status.as_u16(),
            detail: self.detail(),
            instance: None,
        };
        let mut resp = (status, Json(body)).into_response();
        // RFC 9457 strongly recommends this content type.
        resp.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/problem+json"),
        );
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inv_core::state::TransitionError;

    #[test]
    fn validation_maps_to_400() {
        let e: ApiError = CoreError::Validation("nope".into()).into();
        assert_eq!(e.status(), StatusCode::BAD_REQUEST);
        assert_eq!(e.slug(), "validation");
    }

    #[test]
    fn fsm_maps_to_409() {
        let e: ApiError = CoreError::FsmTransition(TransitionError::Illegal {
            state: inv_core::domain::invoice::InvoiceState::Issued,
            event: "issue",
        })
        .into();
        assert_eq!(e.status(), StatusCode::CONFLICT);
    }

    #[test]
    fn not_implemented_maps_to_501() {
        let e: ApiError = CoreError::NotImplemented("link://".into()).into();
        assert_eq!(e.status(), StatusCode::NOT_IMPLEMENTED);
    }
}
