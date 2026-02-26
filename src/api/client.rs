//! Client-facing API (port 8080) — the endpoint clients and agents talk to.
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
/// Returns an OpenAI-compatible model list so clients can enumerate what
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
            "lm_gateway": { "resolves_to": target },
        })
    });

    let data: Vec<Value> = tiers.chain(aliases).collect();
    Json(json!({ "object": "list", "data": data }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use serde_json::json;
    use tower::ServiceExt; // oneshot
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::{
        config::{BackendConfig, Config, GatewayConfig, ProfileConfig, RoutingMode, TierConfig},
        router::RouterState,
        traffic::TrafficLog,
    };

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn minimal_state() -> Arc<RouterState> {
        state_with_backend("http://127.0.0.1:0") // unreachable — only for non-routing tests
    }

    fn state_with_backend(base_url: &str) -> Arc<RouterState> {
        let config = Config {
            gateway: GatewayConfig {
                client_port: 8080,
                admin_port: 8081,
                traffic_log_capacity: 100,
                log_level: None,
            },
            backends: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "mock".into(),
                    BackendConfig {
                        base_url: base_url.into(),
                        api_key_env: None,
                        timeout_ms: 5_000,
                    },
                );
                m
            },
            tiers: vec![
                TierConfig {
                    name: "local:fast".into(),
                    backend: "mock".into(),
                    model: "fast-model".into(),
                },
                TierConfig {
                    name: "cloud:economy".into(),
                    backend: "mock".into(),
                    model: "economy-model".into(),
                },
            ],
            aliases: {
                let mut m = std::collections::HashMap::new();
                m.insert("hint:fast".into(), "local:fast".into());
                m
            },
            profiles: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "default".into(),
                    ProfileConfig {
                        mode: RoutingMode::Dispatch,
                        classifier: "local:fast".into(),
                        max_auto_tier: "cloud:economy".into(),
                        expert_requires_flag: false,
                    },
                );
                m
            },
        };
        Arc::new(RouterState::new(
            Arc::new(config),
            Arc::new(TrafficLog::new(100)),
        ))
    }

    async fn body_json(body: Body) -> serde_json::Value {
        let bytes = to_bytes(body, usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    // -----------------------------------------------------------------------
    // GET /healthz
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn healthz_returns_200_ok() {
        let app = super::router(minimal_state());
        let req = Request::builder()
            .method("GET")
            .uri("/healthz")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["status"], "ok");
    }

    // -----------------------------------------------------------------------
    // GET /v1/models
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn list_models_returns_all_tiers() {
        let app = super::router(minimal_state());
        let req = Request::builder()
            .method("GET")
            .uri("/v1/models")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let json = body_json(resp.into_body()).await;
        assert_eq!(json["object"], "list");
        let data = json["data"].as_array().unwrap();
        let ids: Vec<&str> = data
            .iter()
            .filter_map(|v| v["id"].as_str())
            .collect();
        assert!(ids.contains(&"local:fast"), "missing local:fast: {ids:?}");
        assert!(ids.contains(&"cloud:economy"), "missing cloud:economy: {ids:?}");
    }

    #[tokio::test]
    async fn list_models_includes_aliases() {
        let app = super::router(minimal_state());
        let req = Request::builder()
            .method("GET")
            .uri("/v1/models")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let json = body_json(resp.into_body()).await;
        let data = json["data"].as_array().unwrap();
        let alias_entry = data.iter().find(|v| v["id"] == "hint:fast");
        assert!(alias_entry.is_some(), "alias hint:fast not in model list");
        assert_eq!(alias_entry.unwrap()["owned_by"], "alias");
        assert!(alias_entry.unwrap()["lm_gateway"]["resolves_to"].is_string());
    }

    // -----------------------------------------------------------------------
    // POST /v1/chat/completions
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn chat_completions_proxies_to_backend_and_returns_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{ "message": { "content": "This is a long enough answer from the mock backend to satisfy the sufficiency check." } }]
            })))
            .mount(&server)
            .await;

        let app = super::router(state_with_backend(&server.uri()));
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::to_vec(
                    &json!({ "model": "local:fast", "messages": [{"role": "user", "content": "hello"}] }),
                )
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert!(json.pointer("/choices/0/message/content").is_some());
    }

    #[tokio::test]
    async fn chat_completions_returns_500_when_backend_is_unreachable() {
        // Port 1 is reserved and never responds — guaranteed connection refusal.
        let app = super::router(state_with_backend("http://127.0.0.1:1"));
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::to_vec(
                    &json!({ "model": "local:fast", "messages": [] }),
                )
                .unwrap(),
            ))
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let json = body_json(resp.into_body()).await;
        assert!(json["error"].is_string());
    }
}
