//! Client-facing API (port 8080) — the endpoint ZeroClaw agents talk to.
//!
//! This is intentionally a thin layer: all routing logic lives in [`crate::router`].
//! Handlers translate HTTP concerns (status codes, JSON bodies) into calls
//! to the router and back.

use std::sync::Arc;

use axum::{
    extract::State,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};

use crate::{error::AppError, router::RouterState};

/// Build the client-facing axum router (port 8080).
pub fn router(state: Arc<RouterState>) -> Router {
    Router::new()
        .route("/healthz", get(crate::api::health::healthz))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .with_state(state)
}

/// `POST /v1/chat/completions` — route a chat request through the tier ladder.
///
/// The `model` field in the request body selects the tier or alias. The router
/// rewrites it to the backend's actual model name before forwarding.
pub async fn chat_completions(
    State(state): State<Arc<RouterState>>,
    Json(body): Json<Value>,
) -> Result<impl IntoResponse, AppError> {
    let (resp, _entry) = crate::router::route(&state, body, None, false).await?;
    Ok(Json(resp))
}

/// `GET /v1/models` — list available tiers and aliases as model objects.
///
/// Returns an OpenAI-compatible model list so ZeroClaw can enumerate what
/// routing targets are available without any out-of-band config.
pub async fn list_models(State(state): State<Arc<RouterState>>) -> impl IntoResponse {
    let tiers = state.config.tiers.iter().map(|t| {
        json!({
            "id": t.name,
            "object": "model",
            "owned_by": t.backend,
        })
    });

    let aliases = state.config.aliases.iter().map(|(alias, target)| {
        json!({
            "id": alias,
            "object": "model",
            "owned_by": "alias",
            "claw_router": { "resolves_to": target },
        })
    });

    let data: Vec<Value> = tiers.chain(aliases).collect();
    Json(json!({ "object": "list", "data": data }))
}

