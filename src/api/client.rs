//! Client-facing API (port 8080) — the endpoint ZeroClaw agents talk to.
//!
//! This is intentionally a thin layer: all routing logic lives in [`crate::router`].
//! Handlers translate HTTP concerns (status codes, JSON bodies) into calls
//! to the router and back.

use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};

use crate::router::RouterState;

/// Build the client-facing axum router (port 8080)
pub fn router(state: Arc<RouterState>) -> Router {
    Router::new()
        .route("/healthz", get(crate::api::health::healthz))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .with_state(state)
}

/// POST /v1/chat/completions — proxy to the selected backend/tier
pub async fn chat_completions(
    State(state): State<Arc<RouterState>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    match crate::router::route(&state, body, None, false).await {
        Ok((resp, _entry)) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

/// GET /v1/models — returns configured tiers as model objects
pub async fn list_models(State(state): State<Arc<RouterState>>) -> impl IntoResponse {
    let models: Vec<Value> = state
        .config
        .tiers
        .iter()
        .map(|t| {
            json!({
                "id": t.name,
                "object": "model",
                "owned_by": t.backend,
            })
        })
        .collect();

    // Also include alias names pointing to their real tier
    let mut alias_models: Vec<Value> = state
        .config
        .aliases
        .iter()
        .map(|(alias, target)| {
            json!({
                "id": alias,
                "object": "model",
                "owned_by": "alias",
                "claw_router": { "resolves_to": target }
            })
        })
        .collect();

    let mut all = models;
    all.append(&mut alias_models);

    Json(json!({ "object": "list", "data": all }))
}
