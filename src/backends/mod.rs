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
