//! Admin API (port 8081) — operator-facing introspection endpoints.
//!
//! These endpoints are separated onto a different port so they can be
//! network-restricted independently of the client API (e.g. accessible only
//! from the internal Docker network, never exposed to the internet).

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{backends::BackendClient, router::RouterState};

/// Build the admin-facing axum router (port 8081).
pub fn router(state: Arc<RouterState>) -> Router {
    Router::new()
        .route("/admin/health", get(health))
        .route("/admin/traffic", get(traffic))
        .route("/admin/config", get(config))
        .route("/admin/backends/health", get(backends_health))
        .with_state(state)
}

/// GET /admin/health — checks liveness + optional backend probes
pub async fn health(State(state): State<Arc<RouterState>>) -> impl IntoResponse {
    let tier_count = state.config.tiers.len();
    let backend_count = state.config.backends.len();
    Json(json!({
        "status": "ok",
        "tiers": tier_count,
        "backends": backend_count,
    }))
}

#[derive(Deserialize)]
pub struct TrafficQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}
fn default_limit() -> usize {
    100
}

/// GET /admin/traffic?limit=N — recent N traffic entries (default 100)
pub async fn traffic(
    State(state): State<Arc<RouterState>>,
    Query(q): Query<TrafficQuery>,
) -> impl IntoResponse {
    let entries = state.traffic.recent(q.limit).await;
    let stats = state.traffic.stats().await;
    Json(json!({
        "stats": stats,
        "entries": entries,
    }))
}

/// GET /admin/config — returns the current config with secrets redacted
pub async fn config(State(state): State<Arc<RouterState>>) -> impl IntoResponse {
    let cfg = &state.config;

    // Redact secrets — show env var name but not its resolved value
    let backends: Vec<Value> = cfg
        .backends
        .iter()
        .map(|(name, b)| {
            json!({
                "name": name,
                "base_url": b.base_url,
                "api_key_env": b.api_key_env,
            })
        })
        .collect();

    let tiers: Vec<Value> = cfg
        .tiers
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "backend": t.backend,
                "model": t.model,
            })
        })
        .collect();

    let profiles: Value = cfg
        .profiles
        .iter()
        .map(|(name, p)| {
            (
                name.clone(),
                json!({
                    "mode": p.mode.to_string(),
                    "classifier": p.classifier,
                    "max_auto_tier": p.max_auto_tier,
                    "expert_requires_flag": p.expert_requires_flag,
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into();

    Json(json!({
        "gateway": {
            "client_port": cfg.gateway.client_port,
            "admin_port": cfg.gateway.admin_port,
            "traffic_log_capacity": cfg.gateway.traffic_log_capacity,
        },
        "backends": backends,
        "tiers": tiers,
        "aliases": cfg.aliases,
        "profiles": profiles,
    }))
}

/// GET /admin/backends/health — probe every configured backend
pub async fn backends_health(State(state): State<Arc<RouterState>>) -> impl IntoResponse {
    let mut results = Vec::new();

    for (name, backend_cfg) in &state.config.backends {
        let client = match BackendClient::new(backend_cfg) {
            Ok(c) => c,
            Err(e) => {
                results.push(json!({
                    "backend": name,
                    "status": "error",
                    "error": e.to_string(),
                }));
                continue;
            }
        };

        match client.health_check().await {
            Ok(_) => results.push(json!({ "backend": name, "status": "ok" })),
            Err(e) => results.push(json!({
                "backend": name,
                "status": "unreachable",
                "error": e.to_string(),
            })),
        }
    }

    let all_ok = results.iter().all(|r| r["status"] == "ok");
    let status = if all_ok {
        StatusCode::OK
    } else {
        StatusCode::MULTI_STATUS
    };

    (status, Json(json!({ "backends": results })))
}
