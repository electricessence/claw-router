//! OpenAI-compatible chat completions adapter.
//!
//! Handles any backend that speaks the OpenAI `/v1/chat/completions` protocol —
//! including OpenRouter, LM Studio, vLLM, LocalAI, and others. The request body
//! is forwarded verbatim; no schema translation is performed.

use std::time::Duration;

use anyhow::Context;
use reqwest::{Client, header};
use serde_json::Value;

/// Adapter for any OpenAI-compatible backend.
///
/// Constructed once per request-routing operation; [`Client`] is cheaply
/// clonable internally (it wraps an `Arc`) so there is no meaningful overhead.
pub struct OpenAIAdapter {
    client: Client,
    base_url: String,
}

impl OpenAIAdapter {
    /// Build an adapter for the given base URL and optional bearer token.
    pub fn new(base_url: String, timeout_ms: u64, api_key: Option<String>) -> Self {
        let mut headers = header::HeaderMap::new();
        if let Some(key) = api_key {
            let value = format!("Bearer {key}");
            // Panics on invalid header bytes — surfaces misconfiguration at startup, not at request time.
            headers.insert(
                header::AUTHORIZATION,
                header::HeaderValue::from_str(&value)
                    .expect("API key contains invalid Authorization header characters"),
            );
        }

        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .expect("failed to build reqwest client");

        Self { client, base_url }
    }

    /// Forward a chat completions request to `POST /v1/chat/completions`.
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
            anyhow::bail!("backend returned HTTP {status}: {text}");
        }

        serde_json::from_str(&text)
            .with_context(|| format!("parsing backend response as JSON: {text}"))
    }

    /// Probe the backend with `GET /v1/models`.
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
