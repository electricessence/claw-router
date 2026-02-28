//! Request ID middleware for lm-gateway.
//!
//! Every inbound request is assigned a unique `X-Request-ID`. The ID is:
//!
//! - Accepted from the caller if they already provide `X-Request-ID`
//! - Freshly generated (UUID v4) otherwise
//! - Stored as an axum [`Extension`] so handlers can read it
//! - Echoed back in the `X-Request-ID` response header
//! - Wrapped in a [`tracing`] span so every log line for the request includes it
//!
//! This ties together the admin traffic view (`/admin/traffic`), server logs,
//! and the client response through a single identifier.

use axum::{extract::Request, http::HeaderValue, middleware::Next, response::Response};
use tracing::Instrument as _;
use uuid::Uuid;

/// Newtype wrapper carrying the assigned request ID.
///
/// Exposed as an axum [`Extension`] so any handler can read it:
/// ```rust,ignore
/// async fn handler(Extension(req_id): Extension<RequestId>) { ... }
/// ```
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

/// Axum middleware that assigns a [`RequestId`] to every request.
///
/// Layer order matters: apply this middleware **inside** the
/// `tower_http::TraceLayer` so it runs within the trace span.
pub async fn request_id_middleware(mut req: Request, next: Next) -> Response {
    let id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(String::from)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    req.extensions_mut().insert(RequestId(id.clone()));

    // Wrap the downstream handler in a span so every log line includes the ID.
    let span = tracing::debug_span!("request_id", id = %id);
    let mut response = next.run(req).instrument(span).await;

    if let Ok(header_value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert("x-request-id", header_value);
    }

    response
}
