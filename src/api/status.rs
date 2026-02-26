//! Public status endpoint (`GET /status`, port 8080).
//!
//! Safe to expose publicly without authentication.
//! Returns gateway liveness and aggregate metrics only.
//!
//! What this endpoint **does not** include:
//! - Backend names or URLs
//! - Tier or model names
//! - Routing configuration
//! - Any value that could reveal internal infrastructure
//!
//! This endpoint is enabled by default and intended to be the one public
//! window into the gateway's health. A future admin dashboard requiring
//! HTTPS will offer deeper introspection.

use std::sync::Arc;

use axum::{extract::State, response::IntoResponse, Json};
use serde_json::json;

use crate::router::RouterState;

/// `GET /status` — public liveness and metrics endpoint.
///
/// Example response:
/// ```json
/// {
///   "status": "ok",
///   "uptime_secs": 3600,
///   "requests": {
///     "total": 1024,
///     "errors": 3,
///     "error_rate": 0.003,
///     "escalations": 42,
///     "avg_latency_ms": 87.4
///   }
/// }
/// ```
pub async fn status(State(state): State<Arc<RouterState>>) -> impl IntoResponse {
    let uptime_secs = state.started_at.elapsed().as_secs();
    let stats = state.traffic.public_stats().await;
    let error_rate = if stats.total_requests == 0 {
        0.0_f64
    } else {
        stats.error_count as f64 / stats.total_requests as f64
    };

    Json(json!({
        "status": "ok",
        "uptime_secs": uptime_secs,
        "requests": {
            "total": stats.total_requests,
            "errors": stats.error_count,
            "error_rate": error_rate,
            "escalations": stats.escalation_count,
            "avg_latency_ms": stats.avg_latency_ms,
        }
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    use crate::{
        config::{BackendConfig, Config, GatewayConfig, ProfileConfig, RoutingMode, TierConfig},
        router::RouterState,
        traffic::{TrafficEntry, TrafficLog},
    };

    fn minimal_state() -> Arc<RouterState> {
        let config = Config {
            gateway: GatewayConfig {
                client_port: 8080,
                admin_port: 8081,
                traffic_log_capacity: 100,
                log_level: None,
            },
            backends: std::collections::HashMap::new(),
            tiers: vec![TierConfig {
                name: "local:fast".into(),
                backend: "mock".into(),
                model: "fast-model".into(),
            }],
            aliases: std::collections::HashMap::new(),
            profiles: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "default".into(),
                    ProfileConfig {
                        mode: RoutingMode::Escalate,
                        classifier: "local:fast".into(),
                        max_auto_tier: "local:fast".into(),
                        expert_requires_flag: false,
                    },
                );
                m
            },
        };
        Arc::new(RouterState::new(Arc::new(config), Arc::new(TrafficLog::new(100))))
    }

    #[tokio::test]
    async fn status_returns_ok_with_zero_metrics_on_fresh_state() {
        let app = crate::api::client::router(minimal_state());
        let req = Request::builder()
            .method("GET")
            .uri("/status")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(json["status"], "ok");
        assert_eq!(json["requests"]["total"], 0);
        assert_eq!(json["requests"]["errors"], 0);
        assert_eq!(json["requests"]["error_rate"], 0.0);
    }

    #[tokio::test]
    async fn status_counts_errors_and_computes_error_rate() {
        let state = minimal_state();
        state.traffic.push(TrafficEntry::new("local:fast".into(), "mock".into(), 50, true));
        state.traffic.push(TrafficEntry::new("local:fast".into(), "mock".into(), 80, false));
        state.traffic.push(TrafficEntry::new("local:fast".into(), "mock".into(), 60, false));

        let app = crate::api::client::router(Arc::clone(&state));
        let req = Request::builder()
            .method("GET")
            .uri("/status")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(json["requests"]["total"], 3);
        assert_eq!(json["requests"]["errors"], 2);
        // 2/3 ≈ 0.666…
        let rate = json["requests"]["error_rate"].as_f64().unwrap();
        assert!((rate - 2.0 / 3.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn status_response_contains_no_backend_or_tier_names() {
        let state = minimal_state();
        state.traffic.push(TrafficEntry::new("local:fast".into(), "mock".into(), 50, true));

        let app = crate::api::client::router(Arc::clone(&state));
        let req = Request::builder()
            .method("GET")
            .uri("/status")
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(bytes.to_vec()).unwrap();

        // Must not contain any tier/backend name strings
        assert!(!body.contains("local:fast"), "tier name must not appear in /status");
        assert!(!body.contains("mock"), "backend name must not appear in /status");
    }
}
