//! HTTP client wrapper for a single LLM backend.
//!
//! A [`BackendClient`] is built fresh per request from [`BackendConfig`] — this
//! keeps the lifetime simple and lets each request honour the configured timeout
//! without shared mutable state.

use std::time::Duration;

use anyhow::Context;
use reqwest::{Client, header};
use serde_json::Value;

use crate::config::BackendConfig;

/// HTTP client that forwards requests to a single LLM backend.
///
/// Built from a [`BackendConfig`] via [`BackendClient::new`]. Because
/// [`reqwest::Client`] is cheap to clone (it holds an `Arc` internally),
/// constructing one per request is acceptable and keeps the routing path
/// stateless.
pub struct BackendClient {
    client: Client,
    base_url: String,
}

impl BackendClient {
    /// Construct a client for the given backend config.
    ///
    /// Resolves the API key from the environment variable named in `cfg.api_key_env`
    /// (if any) and injects it as a static `Authorization: Bearer ...` header.
    pub fn new(cfg: &BackendConfig) -> anyhow::Result<Self> {
        let mut headers = header::HeaderMap::new();

        // Add Authorization header if a key is configured
        if let Some(key) = cfg.api_key() {
            let value = format!("Bearer {}", key);
            headers.insert(
                header::AUTHORIZATION,
                header::HeaderValue::from_str(&value)
                    .context("invalid API key value for Authorization header")?,
            );
        }

        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_millis(cfg.timeout_ms))
            .build()
            .context("building reqwest client")?;

        Ok(Self {
            client,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
        })
    }

    /// Forward a `/v1/chat/completions` request body to this backend.
    ///
    /// The `body` should already have `model` and `stream` rewritten by the
    /// router before this is called.
    ///
    /// # Errors
    /// Returns an error if the network request fails, the backend returns a
    /// non-2xx status, or the response body is not valid JSON.
    pub async fn chat_completions(&self, body: Value) -> anyhow::Result<Value> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;

        let status = response.status();
        let text = response.text().await.context("reading response body")?;

        if !status.is_success() {
            anyhow::bail!("backend returned HTTP {}: {}", status, text);
        }

        serde_json::from_str(&text)
            .with_context(|| format!("parsing backend response as JSON: {text}"))
    }

    /// Probe this backend with a lightweight `GET /v1/models` request.
    ///
    /// Used by the admin `/admin/backends/health` endpoint to report readiness
    /// without routing real traffic.
    pub async fn health_check(&self) -> anyhow::Result<()> {
        let url = format!("{}/v1/models", self.base_url);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;

        anyhow::ensure!(
            response.status().is_success(),
            "health check returned HTTP {}",
            response.status()
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn cfg_for(server: &MockServer) -> BackendConfig {
        BackendConfig {
            base_url: server.uri(),
            api_key_env: None,
            timeout_ms: 5_000,
        }
    }

    fn ok_completion_body() -> serde_json::Value {
        json!({
            "choices": [{
                "message": {
                    "content": "Here is a comprehensive response that is definitely long enough."
                }
            }]
        })
    }

    // -----------------------------------------------------------------------
    // BackendClient::new
    // -----------------------------------------------------------------------

    #[test]
    fn new_succeeds_without_api_key() {
        let cfg = BackendConfig {
            base_url: "http://localhost:11434".into(),
            api_key_env: None,
            timeout_ms: 5_000,
        };
        assert!(BackendClient::new(&cfg).is_ok());
    }

    #[test]
    fn new_succeeds_when_configured_api_key_env_var_is_unset() {
        // A missing env var is tolerated; the key is simply omitted from requests.
        let cfg = BackendConfig {
            base_url: "http://localhost:11434".into(),
            api_key_env: Some("CLAW_TEST_DEFINITELY_NOT_SET_XYZ_99".into()),
            timeout_ms: 5_000,
        };
        assert!(BackendClient::new(&cfg).is_ok());
    }

    #[test]
    fn new_resolves_api_key_from_env_var() {
        // Use a unique var name to avoid cross-test interference.
        let var = "CLAW_BACKEND_TEST_KEY_RESOLVE_123";
        // SAFETY: single-threaded test setup; env mutation is acceptable here.
        unsafe { std::env::set_var(var, "sk-test-resolved") };
        let cfg = BackendConfig {
            base_url: "http://localhost:11434".into(),
            api_key_env: Some(var.into()),
            timeout_ms: 5_000,
        };
        let resolved = cfg.api_key();
        assert_eq!(resolved.as_deref(), Some("sk-test-resolved"));
        unsafe { std::env::remove_var(var) };
    }

    #[test]
    fn api_key_returns_none_when_env_var_field_is_none() {
        let cfg = BackendConfig {
            base_url: "http://x".into(),
            api_key_env: None,
            timeout_ms: 5_000,
        };
        assert!(cfg.api_key().is_none());
    }

    // -----------------------------------------------------------------------
    // chat_completions
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn chat_completions_returns_parsed_json_on_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_completion_body()))
            .mount(&server)
            .await;

        let client = BackendClient::new(&cfg_for(&server)).unwrap();
        let result = client
            .chat_completions(json!({"model": "test", "messages": []}))
            .await;

        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert!(result.unwrap().pointer("/choices/0/message/content").is_some());
    }

    #[tokio::test]
    async fn chat_completions_errors_on_non_2xx_status() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
            .mount(&server)
            .await;

        let err = BackendClient::new(&cfg_for(&server))
            .unwrap()
            .chat_completions(json!({"model": "test", "messages": []}))
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains("429"),
            "expected HTTP 429 in error, got: {err}"
        );
    }

    #[tokio::test]
    async fn chat_completions_errors_on_invalid_json_response_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not valid json {{{{"))
            .mount(&server)
            .await;

        let err = BackendClient::new(&cfg_for(&server))
            .unwrap()
            .chat_completions(json!({"model": "test", "messages": []}))
            .await
            .unwrap_err();

        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("json") || msg.contains("parsing"),
            "expected json parse error, got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // health_check
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn health_check_returns_ok_on_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "object": "list", "data": [] })),
            )
            .mount(&server)
            .await;

        assert!(
            BackendClient::new(&cfg_for(&server))
                .unwrap()
                .health_check()
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn health_check_errors_on_non_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let err = BackendClient::new(&cfg_for(&server))
            .unwrap()
            .health_check()
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains("503"),
            "expected HTTP 503 in error, got: {err}"
        );
    }
}
