//! Anthropic Messages API adapter.
//!
//! Translates between the OpenAI chat completions schema (used internally by
//! claw-router) and Anthropic's [`/v1/messages`](https://docs.anthropic.com/en/api/messages)
//! API. Callers route requests as normal OpenAI-format JSON; this adapter
//! handles the schema differences transparently.
//!
//! # Protocol differences handled here
//!
//! | Concern | OpenAI | Anthropic |
//! |---|---|---|
//! | System prompt | First message with `role: "system"` | Top-level `system` field |
//! | Max tokens | Optional (`max_tokens`) | **Required** (`max_tokens`) |
//! | Finish reasons | `"stop"`, `"length"` | `"end_turn"`, `"max_tokens"` |
//! | Response shape | `choices[].message.content` | `content[].text` |
//! | Auth header | `Authorization: Bearer …` | `x-api-key: …` |

use std::time::Duration;

use anyhow::Context;
use reqwest::{Client, header};
use serde_json::{json, Value};

/// Default max_tokens when the caller omits it. Required by Anthropic; sensible
/// ceiling for most conversational use-cases.
const DEFAULT_MAX_TOKENS: u64 = 8_192;

/// Anthropic API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Adapter for the Anthropic Messages API.
pub struct AnthropicAdapter {
    client: Client,
    base_url: String,
}

impl AnthropicAdapter {
    /// Build an Anthropic adapter with the given API key.
    pub fn new(base_url: String, timeout_ms: u64, api_key: String) -> Self {
        let mut headers = header::HeaderMap::new();

        headers.insert(
            "x-api-key",
            header::HeaderValue::from_str(&api_key)
                .expect("Anthropic API key contains invalid header characters"),
        );
        headers.insert(
            "anthropic-version",
            header::HeaderValue::from_static(ANTHROPIC_VERSION),
        );

        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .expect("failed to build reqwest client");

        Self { client, base_url }
    }

    /// Translate and forward a chat completions request to `POST /v1/messages`,
    /// then translate the response back to the OpenAI schema.
    pub async fn chat_completions(&self, request: Value) -> anyhow::Result<Value> {
        let anthropic_req = to_anthropic(request)?;
        let url = format!("{}/v1/messages", self.base_url);

        let response = self
            .client
            .post(&url)
            .json(&anthropic_req)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;

        let status = response.status();
        let text = response.text().await.context("reading Anthropic response body")?;

        if !status.is_success() {
            anyhow::bail!("Anthropic returned HTTP {status}: {text}");
        }

        let body: Value = serde_json::from_str(&text)
            .with_context(|| format!("parsing Anthropic response as JSON: {text}"))?;

        from_anthropic(body)
    }

    /// Probe Anthropic with a minimal 1-token request.
    ///
    /// Anthropic has no `/v1/models` endpoint, so a cheap model inference call
    /// is the only reliable way to verify auth + connectivity.
    pub async fn health_check(&self) -> anyhow::Result<()> {
        let probe = json!({
            "model": "claude-haiku-4-5-20251001",
            "max_tokens": 1,
            "messages": [{ "role": "user", "content": "ping" }],
        });

        let url = format!("{}/v1/messages", self.base_url);
        let response = self
            .client
            .post(&url)
            .json(&probe)
            .send()
            .await
            .with_context(|| format!("health check POST {url}"))?;

        anyhow::ensure!(
            response.status().is_success(),
            "Anthropic health check returned HTTP {}",
            response.status()
        );
        Ok(())
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Schema translation — pub(crate) for unit testing
// ──────────────────────────────────────────────────────────────────────────────

/// Convert an OpenAI chat completions request to the Anthropic Messages format.
pub(crate) fn to_anthropic(request: Value) -> anyhow::Result<Value> {
    let model = request["model"]
        .as_str()
        .context("`model` field is required")?
        .to_string();

    let max_tokens = request["max_tokens"]
        .as_u64()
        .unwrap_or(DEFAULT_MAX_TOKENS);

    let raw_messages = request["messages"]
        .as_array()
        .context("`messages` array is required")?;

    // Anthropic treats system content as a top-level field, not a message role.
    // If multiple system messages are present, concatenate them.
    let mut system_parts: Vec<&str> = Vec::new();
    let mut messages: Vec<Value> = Vec::with_capacity(raw_messages.len());

    for msg in raw_messages {
        if msg["role"].as_str() == Some("system") {
            if let Some(content) = msg["content"].as_str() {
                system_parts.push(content);
            }
        } else {
            messages.push(msg.clone());
        }
    }

    let mut req = json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": messages,
    });

    if !system_parts.is_empty() {
        req["system"] = Value::String(system_parts.join("\n\n"));
    }

    // Forward compatible parameters
    if let Some(temp) = request["temperature"].as_f64() {
        req["temperature"] = json!(temp);
    }
    if let Some(stop) = request.get("stop") {
        req["stop_sequences"] = stop.clone();
    }

    Ok(req)
}

/// Convert an Anthropic Messages API response to the OpenAI chat completions schema.
pub(crate) fn from_anthropic(resp: Value) -> anyhow::Result<Value> {
    // Anthropic responses contain a `content` array of typed blocks.
    // Extract the first text block; non-text blocks (tool_use, etc.) are
    // ignored until streaming/tool-call support is added.
    let text = resp["content"]
        .as_array()
        .and_then(|blocks| blocks.iter().find(|b| b["type"] == "text"))
        .and_then(|b| b["text"].as_str())
        .context("no text block in Anthropic response `content` array")?
        .to_string();

    let model = resp["model"].as_str().unwrap_or("unknown");

    let finish_reason = match resp["stop_reason"].as_str().unwrap_or("stop") {
        "end_turn" => "stop",
        "max_tokens" => "length",
        other => other,
    };

    let input_tokens = resp["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let output_tokens = resp["usage"]["output_tokens"].as_u64().unwrap_or(0);

    Ok(json!({
        "id": resp["id"],
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": text },
            "finish_reason": finish_reason,
        }],
        "usage": {
            "prompt_tokens": input_tokens,
            "completion_tokens": output_tokens,
            "total_tokens": input_tokens + output_tokens,
        },
    }))
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── to_anthropic ──────────────────────────────────────────────────────────

    #[test]
    fn to_anthropic_extracts_system_message_to_top_level() {
        let req = json!({
            "model": "claude-haiku-4-5-20251001",
            "messages": [
                { "role": "system", "content": "You are a helpful assistant." },
                { "role": "user",   "content": "Hello" },
            ],
        });
        let out = to_anthropic(req).unwrap();

        assert_eq!(out["system"], "You are a helpful assistant.");

        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1, "system message should be removed from messages array");
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn to_anthropic_concatenates_multiple_system_messages() {
        let req = json!({
            "model": "claude-haiku-4-5-20251001",
            "messages": [
                { "role": "system", "content": "Part one." },
                { "role": "system", "content": "Part two." },
                { "role": "user",   "content": "Hello" },
            ],
        });
        let out = to_anthropic(req).unwrap();
        assert_eq!(out["system"], "Part one.\n\nPart two.");
    }

    #[test]
    fn to_anthropic_defaults_max_tokens_when_absent() {
        let req = json!({
            "model": "claude-haiku-4-5-20251001",
            "messages": [{ "role": "user", "content": "Hi" }],
        });
        let out = to_anthropic(req).unwrap();
        assert_eq!(out["max_tokens"], DEFAULT_MAX_TOKENS);
    }

    #[test]
    fn to_anthropic_uses_caller_max_tokens() {
        let req = json!({
            "model": "claude-haiku-4-5-20251001",
            "max_tokens": 256,
            "messages": [{ "role": "user", "content": "Hi" }],
        });
        let out = to_anthropic(req).unwrap();
        assert_eq!(out["max_tokens"], 256);
    }

    #[test]
    fn to_anthropic_forwards_temperature() {
        let req = json!({
            "model": "claude-haiku-4-5-20251001",
            "messages": [{ "role": "user", "content": "Hi" }],
            "temperature": 0.3,
        });
        let out = to_anthropic(req).unwrap();
        assert!((out["temperature"].as_f64().unwrap() - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn to_anthropic_errors_without_model() {
        let req = json!({ "messages": [] });
        assert!(to_anthropic(req).is_err());
    }

    #[test]
    fn to_anthropic_errors_without_messages() {
        let req = json!({ "model": "claude-haiku-4-5-20251001" });
        assert!(to_anthropic(req).is_err());
    }

    // ── from_anthropic ────────────────────────────────────────────────────────

    #[test]
    fn from_anthropic_maps_end_turn_to_stop() {
        let resp = json!({
            "id": "msg_123",
            "model": "claude-haiku-4-5-20251001",
            "content": [{ "type": "text", "text": "Hello!" }],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 10, "output_tokens": 5 },
        });
        let out = from_anthropic(resp).unwrap();

        assert_eq!(out["choices"][0]["finish_reason"], "stop");
        assert_eq!(out["choices"][0]["message"]["content"], "Hello!");
        assert_eq!(out["usage"]["prompt_tokens"], 10);
        assert_eq!(out["usage"]["completion_tokens"], 5);
        assert_eq!(out["usage"]["total_tokens"], 15);
    }

    #[test]
    fn from_anthropic_maps_max_tokens_stop_reason_to_length() {
        let resp = json!({
            "id": "msg_456",
            "model": "claude-haiku-4-5-20251001",
            "content": [{ "type": "text", "text": "…" }],
            "stop_reason": "max_tokens",
            "usage": { "input_tokens": 100, "output_tokens": 1024 },
        });
        let out = from_anthropic(resp).unwrap();
        assert_eq!(out["choices"][0]["finish_reason"], "length");
    }

    #[test]
    fn from_anthropic_errors_when_no_text_block_present() {
        let resp = json!({
            "id": "msg_789",
            "model": "claude-haiku-4-5-20251001",
            "content": [{
                "type": "tool_use",
                "id": "toolu_1",
                "name": "calculator",
                "input": {},
            }],
            "stop_reason": "tool_use",
            "usage": { "input_tokens": 10, "output_tokens": 5 },
        });
        assert!(from_anthropic(resp).is_err());
    }

    #[test]
    fn from_anthropic_preserves_message_id() {
        let resp = json!({
            "id": "msg_abc",
            "model": "claude-haiku-4-5-20251001",
            "content": [{ "type": "text", "text": "Hi" }],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 1, "output_tokens": 1 },
        });
        let out = from_anthropic(resp).unwrap();
        assert_eq!(out["id"], "msg_abc");
    }
}
