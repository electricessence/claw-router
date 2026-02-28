//! Request routing logic — the brain of lm-gateway.
//!
//! Two routing modes are supported:
//!
//! - **Dispatch** (`RoutingMode::Dispatch`): a fast local classifier determines
//!   the appropriate tier up-front, then the request is forwarded there directly.
//!   Predictable latency, no wasted backend calls.
//!
//! - **Escalate** (`RoutingMode::Escalate`): the cheapest tier is tried first.
//!   If the response passes the [`is_sufficient`] heuristic it is returned;
//!   otherwise the next tier up is tried. This minimises cost for simple queries
//!   at the expense of higher tail latency on hard ones.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use anyhow::Context;
use serde_json::Value;
use tracing::{debug, warn};

use crate::{
    api::rate_limit::RateLimiter,
    backends::{BackendClient, SseStream},
    config::{Config, RoutingMode, TierConfig},
    traffic::{TrafficEntry, TrafficLog},
};

/// Shared application state injected into every request handler via [`axum::extract::State`].
pub struct RouterState {
    /// Atomically-swappable live config; the lock is held only for the duration
    /// of `Arc::clone`, so it never blocks request handling.
    config_lock: Arc<RwLock<Arc<Config>>>,
    /// Path to the config file on disk — used by the hot-reload background task.
    pub config_path: PathBuf,
    /// In-memory ring-buffer of recent requests, exposed through the admin API.
    pub traffic: Arc<TrafficLog>,
    /// Gateway start time — used to compute uptime for the public status endpoint.
    pub started_at: std::time::Instant,
    /// Optional per-IP rate limiter. `None` means rate limiting is disabled.
    ///
    /// Note: built once at startup from `config.gateway.rate_limit_rpm`.
    /// A config hot-reload will NOT update the rate limiter; restart required
    /// to change the RPM limit at runtime.
    pub rate_limiter: Option<Arc<RateLimiter>>,
    /// Bearer token required for admin API access.
    ///
    /// `None` means admin auth is disabled (port should then be firewalled).
    /// Resolved at startup from `config.gateway.admin_token_env`; not
    /// updated on hot-reload.
    pub admin_token: Option<String>,
    /// Maps resolved client API key values → profile names.
    ///
    /// Built at startup by reading each `[[clients]]` entry's `key_env`.
    /// An empty map means no client key auth is configured — all requests
    /// use the `default` profile (if present) or no profile.
    /// Not updated on hot-reload; restart required to pick up new client keys.
    pub client_map: HashMap<String, String>,
}

impl RouterState {
    pub fn new(config: Arc<Config>, config_path: PathBuf, traffic: Arc<TrafficLog>) -> Self {
        let rate_limiter = config
            .gateway
            .rate_limit_rpm
            .filter(|&rpm| rpm > 0)
            .map(|rpm| Arc::new(RateLimiter::new(rpm)));
        let admin_token = config
            .gateway
            .admin_token_env
            .as_deref()
            .and_then(|var| std::env::var(var).ok())
            .filter(|t| !t.is_empty());
        let client_map: HashMap<String, String> = config
            .clients
            .iter()
            .filter_map(|c| {
                let key = std::env::var(&c.key_env).ok().filter(|k| !k.is_empty())?;
                Some((key, c.profile.clone()))
            })
            .collect();
        if !client_map.is_empty() {
            tracing::info!(count = client_map.len(), "loaded client key mappings");
        }
        Self {
            config_lock: Arc::new(RwLock::new(config)),
            config_path,
            traffic,
            started_at: std::time::Instant::now(),
            rate_limiter,
            admin_token,
            client_map,
        }
    }

    /// Returns a snapshot of the current live config.
    ///
    /// The `RwLock` is held only for the duration of `Arc::clone` (nanoseconds),
    /// so callers get a stable reference with no contention risk.
    pub fn config(&self) -> Arc<Config> {
        self.config_lock.read().expect("config lock poisoned").clone()
    }

    /// Atomically replaces the live config. Called only from the hot-reload task.
    pub fn replace_config(&self, new: Arc<Config>) {
        *self.config_lock.write().expect("config lock poisoned") = new;
    }
}

/// Route a `/v1/chat/completions` request body to the appropriate backend tier.
///
/// - Resolves the `model` field through aliases and tier names.
/// - Selects a routing mode from the active [`ProfileConfig`].
/// - Forwards the (rewritten) request and records a [`TrafficEntry`].
///
/// Returns the raw JSON response from the winning backend, plus the traffic entry
/// so callers can surface per-request metadata (e.g. via response headers).
#[tracing::instrument(
    skip(state, request_body),
    fields(
        profile = profile_name.unwrap_or("default"),
        tier = tracing::field::Empty,
    )
)]
pub async fn route(
    state: &RouterState,
    mut request_body: Value,
    profile_name: Option<&str>,
    request_id: Option<&str>,
    stream: bool,
) -> anyhow::Result<(Value, TrafficEntry)> {
    let profile_name = profile_name.unwrap_or("default");
    let config = state.config();
    let profile = config
        .profile(profile_name)
        .context("no matching profile and no default profile configured")?;

    // Resolve model → tier — clone to a String so we don't hold a borrow into request_body
    let model_hint = request_body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("hint:fast")
        .to_owned();
    let resolved_tier = config.resolve_tier(&model_hint);
    let target_tier: &TierConfig = match resolved_tier {
        Some(tier) => tier,
        None => {
            warn!(%model_hint, "unknown model/alias — falling back to classifier tier");
            config
                .tiers
                .iter()
                .find(|t| t.name == profile.classifier)
                .context("classifier tier not found")?
        }
    };

    tracing::Span::current().record("tier", target_tier.name.as_str());

    let (response, entry) = match profile.mode {
        RoutingMode::Dispatch => {
            dispatch(state, &mut request_body, target_tier, stream).await?
        }
        RoutingMode::Escalate => {
            escalate(state, &mut request_body, profile, stream).await?
        }
    };

    // Enrich entry with request-level context only available at route() scope,
    // then record it in the traffic log.
    let mut entry = entry
        .with_profile(profile_name)
        .with_requested_model(&model_hint)
        .with_routing_mode(match profile.mode {
            RoutingMode::Dispatch => "dispatch",
            RoutingMode::Escalate => "escalate",
        });
    if let Some(id) = request_id {
        entry = entry.with_id(id);
    }

    state.traffic.push(entry.clone());

    Ok((response, entry))
}

/// Mode A: classify up-front and dispatch directly to the resolved tier.
///
/// The request body is mutated in place to rewrite `model` and `stream`
/// before being forwarded — no copy of the full body is made.
async fn dispatch(
    state: &RouterState,
    body: &mut Value,
    tier: &TierConfig,
    stream: bool,
) -> anyhow::Result<(Value, TrafficEntry)> {
    let config = state.config();
    let backend_cfg = config
        .backends
        .get(&tier.backend)
        .with_context(|| format!("backend `{}` not in config", tier.backend))?;

    // Rewrite the model field to the backend's model name
    if let Some(obj) = body.as_object_mut() {
        obj.insert("model".into(), Value::String(tier.model.clone()));
        obj.insert("stream".into(), Value::Bool(stream));
    }

    debug!(tier = %tier.name, backend = %tier.backend, model = %tier.model, "dispatching");

    let client = BackendClient::new(backend_cfg)?;
    let t0 = std::time::Instant::now();
    let response = client.chat_completions(body.clone()).await?;
    let latency_ms = t0.elapsed().as_millis() as u64;

    let entry = TrafficEntry::new(tier.name.clone(), tier.backend.clone(), latency_ms, true);

    Ok((response, entry))
}

/// Mode B: try tiers cheapest-first and return the first sufficient response.
///
/// Iteration stops at `profile.max_auto_tier`. Backend failures and insufficient
/// responses both cause escalation to the next tier. If every tier is exhausted
/// without a sufficient response an error is returned.
async fn escalate(
    state: &RouterState,
    body: &mut Value,
    profile: &crate::config::ProfileConfig,
    stream: bool,
) -> anyhow::Result<(Value, TrafficEntry)> {
    let config = state.config();
    // Collect candidate tiers up to max_auto_tier
    let max_idx = config
        .tiers
        .iter()
        .position(|t| t.name == profile.max_auto_tier)
        .unwrap_or(config.tiers.len() - 1);

    let candidates: Vec<&TierConfig> = config.tiers[..=max_idx].iter().collect();

    for (tier_idx, tier) in candidates.iter().enumerate() {
        let backend_cfg = match config.backends.get(&tier.backend) {
            Some(b) => b,
            None => continue,
        };

        if let Some(obj) = body.as_object_mut() {
            obj.insert("model".into(), Value::String(tier.model.clone()));
            obj.insert("stream".into(), Value::Bool(stream));
        }

        let client = match BackendClient::new(backend_cfg) {
            Ok(c) => c,
            Err(e) => {
                warn!(tier = %tier.name, error = %e, "skipping tier — client build failed");
                continue;
            }
        };

        let t0 = std::time::Instant::now();
        match client.chat_completions(body.clone()).await {
            Ok(response) => {
                let latency_ms = t0.elapsed().as_millis() as u64;
                if is_sufficient(&response) {
                    let mut entry =
                        TrafficEntry::new(tier.name.clone(), tier.backend.clone(), latency_ms, true);
                    if tier_idx > 0 {
                        entry = entry.mark_escalated();
                    }
                    return Ok((response, entry));
                }
                debug!(tier = %tier.name, "response insufficient — escalating");
            }
            Err(e) => {
                warn!(tier = %tier.name, error = %e, "tier request failed — escalating");
            }
        }
    }

    // Exhausted all tiers — last resort: use the final candidate anyway
    anyhow::bail!("all tiers exhausted without a sufficient response")
}

/// Route a streaming `/v1/chat/completions` request.
///
/// Streaming bypasses escalation — the first matching tier is dispatched to
/// directly, and the backend's SSE output is returned as an [`SseStream`].
/// All backends produce OpenAI-compatible SSE: OpenAI-compatible and Ollama
/// backends proxy bytes verbatim; Anthropic translates on-the-fly.
#[tracing::instrument(skip(state, request_body), fields(profile = profile_name.unwrap_or("default")))]
pub async fn route_stream(
    state: &RouterState,
    mut request_body: Value,
    profile_name: Option<&str>,
    request_id: Option<&str>,
) -> anyhow::Result<(SseStream, TrafficEntry)> {
    let profile_name = profile_name.unwrap_or("default");
    let config = state.config();
    let profile = config
        .profile(profile_name)
        .context("no matching profile and no default profile configured")?;

    let model_hint = request_body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("hint:fast")
        .to_owned();

    let resolved_tier = config.resolve_tier(&model_hint);
    let target_tier: &TierConfig = match resolved_tier {
        Some(tier) => tier,
        None => {
            warn!(%model_hint, "unknown model/alias — falling back to classifier tier");
            config
                .tiers
                .iter()
                .find(|t| t.name == profile.classifier)
                .context("classifier tier not found")?
        }
    };

    let backend_cfg = config
        .backends
        .get(&target_tier.backend)
        .with_context(|| format!("backend `{}` not in config", target_tier.backend))?;

    if let Some(obj) = request_body.as_object_mut() {
        obj.insert("model".into(), Value::String(target_tier.model.clone()));
        obj.insert("stream".into(), Value::Bool(true));
    }

    debug!(tier = %target_tier.name, backend = %target_tier.backend, "streaming dispatch");

    let client = BackendClient::new(backend_cfg)?;
    let t0 = std::time::Instant::now();
    let stream_response = client.chat_completions_stream(request_body).await?;
    let latency_ms = t0.elapsed().as_millis() as u64;

    // Latency here is time-to-first-byte (connection + headers), not full response.
    let mut entry = TrafficEntry::new(
        target_tier.name.clone(),
        target_tier.backend.clone(),
        latency_ms,
        true,
    )
    .with_profile(profile_name)
    .with_requested_model(&model_hint)
    .with_routing_mode("stream");
    if let Some(id) = request_id {
        entry = entry.with_id(id);
    }

    state.traffic.push(entry.clone());

    Ok((stream_response, entry))
}

/// Decide whether a backend response is good enough to return or should be escalated.
///
/// This intentionally uses simple, fast heuristics rather than another LLM call:
///
/// - Responses shorter than 20 characters are almost certainly non-answers.
/// - Common refusal phrases indicate the model couldn't help.
///
/// The function is `pub(crate)` so it can be unit-tested without making it part of
/// the public API.
pub(crate) fn is_sufficient(response: &Value) -> bool {
    // Extract the content from the first choice
    let content = response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or("");

    // Escalate if the response is very short (likely a non-answer)
    if content.len() < 20 {
        return false;
    }

    // Escalate if the model explicitly refuses
    let lower = content.to_lowercase();
    let refusal_phrases = [
        "i don't know",
        "i cannot",
        "i'm not able to",
        "as an ai",
        "i don't have enough information",
    ];
    if refusal_phrases.iter().any(|p| lower.contains(p)) {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // is_sufficient — pure heuristic, no I/O required
    // -----------------------------------------------------------------------

    fn response_with_content(content: &str) -> Value {
        json!({
            "choices": [{
                "message": { "content": content }
            }]
        })
    }

    #[test]
    fn sufficient_for_normal_response() {
        let r = response_with_content("Here is a detailed explanation of how Rust lifetimes work.");
        assert!(is_sufficient(&r));
    }

    #[test]
    fn insufficient_when_content_is_very_short() {
        // Under 20 chars — likely a fragment, not a real answer
        assert!(!is_sufficient(&response_with_content("Sure.")));
        assert!(!is_sufficient(&response_with_content("")));
    }

    #[test]
    fn insufficient_when_model_refuses() {
        let refusals = [
            "I cannot help with that request.",
            "As an AI, I must decline to answer.",
            "I don't know the answer to your question.",
            "I'm not able to provide that information.",
            "I don't have enough information to respond accurately.",
        ];
        for phrase in refusals {
            assert!(
                !is_sufficient(&response_with_content(phrase)),
                "expected refusal to be insufficient: {phrase}"
            );
        }
    }

    #[test]
    fn refusal_detection_is_case_insensitive() {
        let r = response_with_content("AS AN AI language model, I cannot do that at all.");
        assert!(!is_sufficient(&r));
    }

    #[test]
    fn insufficient_when_choices_array_is_missing() {
        // Malformed response — treat as insufficient so we try again
        assert!(!is_sufficient(&json!({})));
        assert!(!is_sufficient(&json!({ "choices": [] })));
    }

    // -----------------------------------------------------------------------
    // route() — dispatch and escalate with mock backends
    // -----------------------------------------------------------------------

    use std::sync::Arc;

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::{
        config::{BackendConfig, GatewayConfig, ProfileConfig, RoutingMode, TierConfig},
        traffic::TrafficLog,
    };

    async fn mock_state(server: &MockServer, mode: RoutingMode) -> RouterState {
        let config = crate::config::Config {
            gateway: GatewayConfig {
                client_port: 8080,
                admin_port: 8081,
                traffic_log_capacity: 100,
                log_level: None,
                rate_limit_rpm: None,
                admin_token_env: None,
            },
            backends: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "mock".into(),
                    BackendConfig {
                        base_url: server.uri(),
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
                        mode,
                        classifier: "local:fast".into(),
                        max_auto_tier: "cloud:economy".into(),
                        expert_requires_flag: false,
                    },
                );
                m
            },
        };
        RouterState::new(Arc::new(config), std::path::PathBuf::default(), Arc::new(TrafficLog::new(100)))
    }

    fn long_response(content: &str) -> serde_json::Value {
        json!({
            "choices": [{ "message": { "content": content } }]
        })
    }

    #[tokio::test]
    async fn dispatch_routes_to_resolved_tier_and_returns_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(long_response(
                "Here is a comprehensive answer that passes the sufficiency heuristic.",
            )))
            .mount(&server)
            .await;

        let state = mock_state(&server, RoutingMode::Dispatch).await;
        let body = json!({ "model": "hint:fast", "messages": [{"role": "user", "content": "hi"}] });

        let result = route(&state, body, None, None, false).await;
        assert!(result.is_ok(), "dispatch failed: {:?}", result.err());

        let (resp, entry) = result.unwrap();
        assert!(resp.pointer("/choices/0/message/content").is_some());
        assert_eq!(entry.tier, "local:fast");
        assert_eq!(entry.backend, "mock");
        assert!(entry.success);
    }

    #[tokio::test]
    async fn dispatch_resolves_direct_tier_name_without_alias() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(long_response(
                "Direct tier name resolved correctly to the right backend tier.",
            )))
            .mount(&server)
            .await;

        let state = mock_state(&server, RoutingMode::Dispatch).await;
        let body = json!({ "model": "cloud:economy", "messages": [] });

        let (_, entry) = route(&state, body, None, None, false).await.unwrap();
        assert_eq!(entry.tier, "cloud:economy");
    }

    #[tokio::test]
    async fn escalate_returns_first_sufficient_response() {
        let server = MockServer::start().await;
        // First tier (local:fast) is sufficient
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(long_response(
                "This is a sufficient answer from the cheapest tier, no need to escalate further.",
            )))
            .mount(&server)
            .await;

        let state = mock_state(&server, RoutingMode::Escalate).await;
        let body = json!({ "model": "hint:fast", "messages": [] });

        let (_, entry) = route(&state, body, None, None, false).await.unwrap();
        // Should have stopped at the first (cheapest) tier
        assert_eq!(entry.tier, "local:fast");
    }

    #[tokio::test]
    async fn route_records_entry_in_traffic_log() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(long_response(
                "Traffic log entry should be created for every successful route call.",
            )))
            .mount(&server)
            .await;

        let state = mock_state(&server, RoutingMode::Dispatch).await;
        let body = json!({ "model": "local:fast", "messages": [] });

        route(&state, body, None, None, false).await.unwrap();

        let entries = state.traffic.recent(10).await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].tier, "local:fast");
        assert!(entries[0].success);
    }

    #[tokio::test]
    async fn route_errors_when_no_profile_is_configured() {
        let state = RouterState::new(
            Arc::new(crate::config::Config {
                gateway: GatewayConfig {
                    client_port: 8080,
                    admin_port: 8081,
                    traffic_log_capacity: 10,
                    log_level: None,
                    rate_limit_rpm: None,
                    admin_token_env: None,
                },
                backends: std::collections::HashMap::new(),
                tiers: vec![],
                aliases: std::collections::HashMap::new(),
                profiles: std::collections::HashMap::new(), // no default
            }),
            std::path::PathBuf::default(),
            Arc::new(TrafficLog::new(10)),
        );

        let result = route(&state, json!({}), None, false).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("no matching profile"));
    }

    #[tokio::test]
    async fn dispatch_falls_back_to_classifier_tier_on_unknown_model() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(long_response(
                "Fallback to classifier tier when model hint is unknown.",
            )))
            .mount(&server)
            .await;

        let state = mock_state(&server, RoutingMode::Dispatch).await;
        // "totally:unknown" exists in neither aliases nor tiers — should fall back to classifier
        let body = json!({ "model": "totally:unknown", "messages": [] });

        let (_, entry) = route(&state, body, None, None, false).await.unwrap();
        // classifier is "local:fast"
        assert_eq!(entry.tier, "local:fast");
    }
}
