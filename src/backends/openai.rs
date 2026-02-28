//! OpenAI-compatible chat completions adapter.
//!
//! Handles any backend that speaks the OpenAI `/v1/chat/completions` protocol —
//! including OpenRouter, LM Studio, vLLM, LocalAI, and others. The request body
//! is forwarded verbatim; no schema translation is performed.

use std::time::Duration;

use anyhow::Context;
use futures_util::StreamExt as _;
use reqwest::{Client, header};
use serde_json::Value;

use super::SseStream;

/// Adapter for any OpenAI-compatible backend.
///
/// Constructed once per request-routing operation; [`Client`] is cheaply
/// clonable internally (it wraps an `Arc`) so there is no meaningful overhead.
pub struct OpenAIAdapter {
    /// Buffered requests — has the configured request timeout.
    client: Client,
    /// Streaming requests — no request-level timeout (body arrives incrementally).
    stream_client: Client,
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
            .default_headers(headers.clone())
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .expect("failed to build reqwest client");

        // No request-level timeout for streaming — the response body arrives
        // incrementally. TCP connect timeout still applies.
        let stream_client = Client::builder()
            .default_headers(headers)
            .build()
            .expect("failed to build streaming reqwest client");

        Self { client, stream_client, base_url }
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

    /// Send `POST /v1/chat/completions` and return an [`SseStream`] for proxying.
    ///
    /// The backend response bytes are forwarded verbatim — no buffering, no schema
    /// translation. Uses the no-timeout `stream_client`.
    pub async fn chat_completions_stream(&self, body: Value) -> anyhow::Result<SseStream> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let response = self
            .stream_client
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url} (streaming)"))?;
        let stream = response
            .bytes_stream()
            .map(|r| r.map_err(anyhow::Error::from));
        Ok(Box::pin(stream))
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
