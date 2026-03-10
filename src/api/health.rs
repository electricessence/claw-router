//! Liveness probe endpoint shared across both listeners.

use axum::{http::StatusCode, response::IntoResponse, Json};
use serde_json::json;

/// `GET /healthz` — always returns 200 OK with `{"status": "ok"}`.
///
/// This endpoint has no dependencies and never blocks, making it safe to use
/// as a Docker / Kubernetes liveness probe.
pub async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}
