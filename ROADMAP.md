# lm-gateway-rs Roadmap

A lightweight, single-binary LLM routing gateway in Rust. No Python. No database. No bloat.

> This is a living document. Items move as priorities clarify.  
> Contributions and discussion welcome — open an issue.

---

## Where We Are Today

**v0.1 — Stable**

- Single binary, zero runtime dependencies
- Transparent multi-backend routing (Anthropic, OpenAI-compatible, Ollama, OpenRouter)
- Tier-based escalation: route cheapest-first, escalate when needed
- Model aliasing: expose simple names (`hint:fast`, `hint:capable`) regardless of backend
- Per-client API keys mapped to named routing profiles
- Profile-level routing policies: mode, classifier tier, max auto-escalation tier
- In-memory traffic log (ring buffer) — no disk I/O
- `GET /status` — zero-leak public metrics (uptime, request counts, error rate)
- `GET /` admin UI — live traffic table, backend health, profiles, config view (no secrets)
- TOML config under 50 lines for a full production setup
- Docker image under 15 MB (`scratch` base, static musl binary)

**v0.2 — Complete**

- **Anthropic streaming**: on-the-fly SSE translation — `stream: true` works end-to-end with Anthropic backends
- **`GET /metrics`**: Prometheus-compatible scrape endpoint (TYPE gauge; ring-buffer windowed stats)
- **Config hot-reload**: `POST /admin/reload` applies config changes without restart; `↺ Reload` button in admin UI
- **Request ID tracing**: `X-Request-ID` propagated or generated per request; matches traffic log entry IDs
- **Per-IP rate limiting**: token bucket on the client port; configurable via `rate_limit_rpm`
- **Admin Bearer token auth**: all admin routes optionally protected via `admin_token_env`
- **Retry / backend failover**: configurable `max_retries` + `retry_delay_ms`; automatic failover to next tier on error
- **Backend health tracking**: recent-window error-rate snapshot; degraded backends skipped during escalation
- **Per-profile rate limits**: shared RPM quota per profile — all keys mapped to the same profile share a single bucket
- **Pluggable secret backends**: `api_key_secret = { source = "env", var = "..." }` or `{ source = "file", path = "..." }` — supports Docker secrets, Kubernetes mounts, any file-based store
- **Admin dashboard improvements**: backend cards show live health + traffic error rate; profiles section; secret source badge (env/file); setup warning banner when keys are unresolved

---

## Short Range

> Targeted next

### `GET /v1/models` — model discovery

Return the list of configured tier names and aliases as a standard OpenAI `/v1/models` response. Most OpenAI-compatible clients call this endpoint on startup to populate their model selector — without it, users must manually configure model names.

```json
{
  "object": "list",
  "data": [
    { "id": "hint:fast",     "object": "model" },
    { "id": "hint:capable",  "object": "model" },
    { "id": "local:fast",    "object": "model" },
    { "id": "cloud:economy", "object": "model" }
  ]
}
```

### Traffic log export

The traffic ring buffer is in-memory only — it disappears on restart. Two opt-in export modes:

- **JSONL append**: write each completed request to a file (`traffic_log_path` in config)
- **Webhook**: POST each entry as JSON to a configurable URL (`traffic_webhook_url`)

Both are optional and fire async so they don't add latency to the request path.

---

## Medium Range

### TLS for the admin port

The admin UI (port 8081) serves over plain HTTP. Fine on a private network; not acceptable across trust boundaries. The recommended pattern is termination via a reverse proxy (Caddy, nginx) — native TLS in the binary is a secondary option.

### Response caching (opt-in, request-scoped)

For deterministic or near-deterministic prompts, cache the response against a hash of the full request (model + messages + sampling params). Configurable TTL per profile. **Disabled by default; never shared across profiles.** Most useful for classification pipelines that ask the same question repeatedly.

---

## Vision

lm-gateway-rs is built on a simple principle: **the deployment model should never become the problem.** One binary. One config file. Zero external state. Runs anywhere.

No Python runtime. No database. No framework you have to understand before you can understand the router. The source is small enough to read in an afternoon, and the config is under 50 lines for a full production setup.

The routing intelligence grows over time — semantic routing based on prompt content, automatic cost/quality tradeoffs, backend reliability tracking. But the shape of the thing stays the same.

---

## Not In Scope

- A database (the traffic log is an in-memory ring buffer by design)
- A Python runtime or scripting layer
- A UI for configuring routing rules (config file is the interface)
- Autonomous model fine-tuning or training
