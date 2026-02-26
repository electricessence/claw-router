//! Ollama adapter.
//!
//! Ollama ships an OpenAI-compatible `/v1/chat/completions` endpoint, so this
//! adapter is intentionally thin — it delegates to the same HTTP path, but
//! handles the keyless-auth case transparently and uses Ollama's root `/`
//! endpoint for health checks rather than `/v1/models`.
//!
//! In the future this adapter can opt into Ollama's native `/api/chat` path
//! to access Ollama-specific features (tool calls, image inputs, etc.) without
//! requiring the compat layer.

use std::time::Duration;

use anyhow::Context;
use reqwest::Client;
use serde_json::Value;

/// Adapter for a locally-running Ollama instance.
pub struct OllamaAdapter {
    client: Client,
    base_url: String,
}

impl OllamaAdapter {
    /// Build an Ollama adapter. No API key is required for typical local deployments.
    pub fn new(base_url: String, timeout_ms: u64) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .expect("failed to build reqwest client");

        Self { client, base_url }
    }

    /// Forward a chat completions request via Ollama's OpenAI-compat endpoint.
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
        let text = response.text().await.context("reading Ollama response body")?;

        if !status.is_success() {
            anyhow::bail!("Ollama returned HTTP {status}: {text}");
        }

        serde_json::from_str(&text)
            .with_context(|| format!("parsing Ollama response as JSON: {text}"))
    }

    /// Probe Ollama's root endpoint (`GET /`) — returns `"Ollama is running"` on success.
    pub async fn health_check(&self) -> anyhow::Result<()> {
        let url = format!("{}/", self.base_url);
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;

        anyhow::ensure!(
            response.status().is_success(),
            "Ollama health check returned HTTP {}",
            response.status()
        );
        Ok(())
    }
}
