//! Anthropic Messages API adapter.
//!
//! Translates between the OpenAI chat completions schema (used internally by
//! lm-gateway) and Anthropic's [`/v1/messages`](https://docs.anthropic.com/en/api/messages)
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
use bytes::Bytes;
use futures_util::StreamExt as _;
use reqwest::{Client, header};
use serde_json::{json, Value};

use super::SseStream;

/// Default max_tokens when the caller omits it. Required by Anthropic; sensible
/// ceiling for most conversational use-cases.
const DEFAULT_MAX_TOKENS: u64 = 8_192;

/// Anthropic API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Adapter for the Anthropic Messages API.
pub struct AnthropicAdapter {
    /// Buffered requests — has the configured request timeout.
    client: Client,
    /// Streaming requests — no request-level timeout.
    stream_client: Client,
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
            .default_headers(headers.clone())
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .expect("failed to build reqwest client");

        let stream_client = Client::builder()
            .default_headers(headers)
            .build()
            .expect("failed to build streaming reqwest client");

        Self { client, stream_client, base_url }
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

    /// Forward a streaming completions request, translating Anthropic SSE events to
    /// OpenAI-compatible format on-the-fly.
    ///
    /// Anthropic's SSE schema (`content_block_delta`, `message_start`, etc.) differs
    /// from OpenAI's (`data: {choices:[{delta:{content:"..."}}]}`). This method spawns
    /// a background task that reads the Anthropic stream, translates each event, and
    /// forwards the translated bytes through a channel as the returned [`SseStream`].
    pub async fn chat_completions_stream(&self, request: Value) -> anyhow::Result<SseStream> {
        let mut anthropic_req = to_anthropic(request)?;
        // Tell Anthropic we want a streamed response.
        if let Some(obj) = anthropic_req.as_object_mut() {
            obj.insert("stream".into(), Value::Bool(true));
        }

        let url = format!("{}/v1/messages", self.base_url);
        let response = self
            .stream_client
            .post(&url)
            .json(&anthropic_req)
            .send()
            .await
            .with_context(|| format!("POST {url} (streaming)"))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic returned HTTP {status}: {text}");
        }

        let (tx, mut rx) = tokio::sync::mpsc::channel::<anyhow::Result<Bytes>>(32);
        let msg_id = uuid::Uuid::new_v4().to_string();

        tokio::spawn(async move {
            let mut byte_stream = response.bytes_stream();
            let mut buf = String::new();
            let mut event_type = String::new();
            let mut model = String::from("unknown");

            while let Some(chunk) = byte_stream.next().await {
                match chunk {
                    Err(e) => {
                        let _ = tx.send(Err(anyhow::anyhow!(e))).await;
                        return;
                    }
                    Ok(bytes) => {
                        buf.push_str(&String::from_utf8_lossy(&bytes));
                        loop {
                            match buf.find('\n') {
                                None => break,
                                Some(pos) => {
                                    let line = buf[..pos].trim_end_matches('\r').to_string();
                                    buf.drain(..=pos);

                                    if line.is_empty() {
                                        event_type.clear();
                                    } else if let Some(val) = line.strip_prefix("event: ") {
                                        event_type = val.to_string();
                                    } else if let Some(data) = line.strip_prefix("data: ") {
                                        if let Some(out) = translate_sse_event(
                                            &event_type, data, &msg_id, &mut model,
                                        ) {
                                            if tx.send(Ok(Bytes::from(out))).await.is_err() {
                                                return; // client disconnected
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let _ = tx.send(Ok(Bytes::from("data: [DONE]\n\n"))).await;
        });

        let stream = futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx));
        Ok(Box::pin(stream))
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
// SSE stream translation — Anthropic → OpenAI format
// ──────────────────────────────────────────────────────────────────────────────

/// Translate a single Anthropic SSE event into an OpenAI-compatible SSE chunk.
///
/// Returns `Some(bytes_to_emit)` for events that map to OpenAI chunks, `None`
/// for Anthropic-specific events that have no OpenAI equivalent (ping,
/// `content_block_start`, `content_block_stop`, `message_stop`).
///
/// `model` is populated from the first `message_start` event and reused for
/// all subsequent chunks.
pub(crate) fn translate_sse_event(
    event_type: &str,
    data: &str,
    msg_id: &str,
    model: &mut String,
) -> Option<String> {
    match event_type {
        "message_start" => {
            // Extract the model name from the first event for use in all chunks.
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                if let Some(m) = v.pointer("/message/model").and_then(Value::as_str) {
                    *model = m.to_string();
                }
            }
            let chunk = json!({
                "id": msg_id,
                "object": "chat.completion.chunk",
                "model": &*model,
                "choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}, "finish_reason": null}],
            });
            Some(format!("data: {chunk}\n\n"))
        }
        "content_block_delta" => {
            let v = serde_json::from_str::<Value>(data).ok()?;
            let text = v.pointer("/delta/text").and_then(Value::as_str)?;
            let chunk = json!({
                "id": msg_id,
                "object": "chat.completion.chunk",
                "model": &*model,
                "choices": [{"index": 0, "delta": {"content": text}, "finish_reason": null}],
            });
            Some(format!("data: {chunk}\n\n"))
        }
        "message_delta" => {
            let v = serde_json::from_str::<Value>(data).ok()?;
            // Map Anthropic stop reasons to OpenAI finish reasons.
            let finish_reason = v
                .pointer("/delta/stop_reason")
                .and_then(Value::as_str)
                .map(|r| match r {
                    "end_turn" => "stop",
                    "max_tokens" => "length",
                    other => other,
                });
            let chunk = json!({
                "id": msg_id,
                "object": "chat.completion.chunk",
                "model": &*model,
                "choices": [{"index": 0, "delta": {}, "finish_reason": finish_reason}],
            });
            Some(format!("data: {chunk}\n\n"))
        }
        // ping, content_block_start, content_block_stop, message_stop → skip
        _ => None,
    }
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

    // ── translate_sse_event ───────────────────────────────────────────────────

    #[test]
    fn translate_message_start_sets_role_and_captures_model() {
        let mut model = String::from("unknown");
        let data = json!({
            "type": "message_start",
            "message": { "model": "claude-3-5-sonnet-20241022" }
        })
        .to_string();
        let out = translate_sse_event("message_start", &data, "id-1", &mut model).unwrap();
        assert_eq!(model, "claude-3-5-sonnet-20241022");
        let chunk: Value = serde_json::from_str(out.trim_start_matches("data: ").trim_end()).unwrap();
        assert_eq!(chunk["choices"][0]["delta"]["role"], "assistant");
        assert_eq!(chunk["choices"][0]["delta"]["content"], "");
    }

    #[test]
    fn translate_content_block_delta_emits_text() {
        let mut model = String::from("claude-3-5-haiku");
        let data = json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "Hello!" }
        })
        .to_string();
        let out = translate_sse_event("content_block_delta", &data, "id-2", &mut model).unwrap();
        let chunk: Value = serde_json::from_str(out.trim_start_matches("data: ").trim_end()).unwrap();
        assert_eq!(chunk["choices"][0]["delta"]["content"], "Hello!");
    }

    #[test]
    fn translate_message_delta_maps_stop_reasons() {
        for (anthropic, openai) in [("end_turn", "stop"), ("max_tokens", "length")] {
            let mut model = String::from("m");
            let data = json!({
                "type": "message_delta",
                "delta": { "stop_reason": anthropic },
            })
            .to_string();
            let out = translate_sse_event("message_delta", &data, "id-3", &mut model).unwrap();
            let chunk: Value =
                serde_json::from_str(out.trim_start_matches("data: ").trim_end()).unwrap();
            assert_eq!(chunk["choices"][0]["finish_reason"], openai);
        }
    }

    #[test]
    fn translate_skips_ping_and_housekeeping_events() {
        let mut model = String::new();
        for event in ["ping", "content_block_start", "content_block_stop", "message_stop"] {
            assert!(
                translate_sse_event(event, "{}", "id", &mut model).is_none(),
                "{event} should be skipped"
            );
        }
    }
}
