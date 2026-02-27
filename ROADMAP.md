# lm-gateway-rs Roadmap

A lightweight, single-binary LLM routing gateway in Rust. No Python. No database. No bloat.

> This is a living document. Items move as priorities clarify.  
> Contributions and discussion welcome — open an issue.

---

## Where We Are Today

**v0.1 — Working**

- Single binary, zero runtime dependencies
- Transparent multi-backend routing (Anthropic, OpenAI-compatible, Ollama)
- Tier-based escalation: route cheapest-first, escalate when response quality is insufficient
- Model aliasing: expose simple names (`hint:fast`, `hint:capable`) regardless of backend
- Profile system: named routing policies per use-case
- In-memory traffic log (ring buffer) — no disk I/O
- `GET /status` — zero-leak public metrics (uptime, request counts, error rate)
- `GET /` admin UI — live traffic table, backend health, config view (no secrets exposed)
- `GET /healthz` — liveness probe for container orchestrators
- TOML config under 50 lines for a full production setup
- Docker image under 15 MB (`scratch` base, static musl binary)

---

## Short Range

> Targeted next

### Per-client API keys + routing profiles

Right now the gateway has one routing identity. The next step is making it multi-tenant: each downstream client presents their own key, and the gateway maps that key to a profile (model set, cost policy, rate limits).

```toml
[[clients]]
key_env = "CLIENT_ACME_KEY"
profile = "economy"

[[clients]]
key_env = "CLIENT_INTERNAL_KEY"
profile = "expert"
```

Use cases: different agents with different budgets, exposing the gateway to a team with per-member keys, charging clients through their own upstream keys.

### Streaming (SSE)

Pass through server-sent event streams from backends that support them. Required for real-time UX — chat interfaces, voice pipelines, anything latency-sensitive.

### `/metrics` — Prometheus scrape endpoint

Expose the ring-buffer stats as a Prometheus-compatible endpoint on the admin port. Pairs with Grafana for production visibility without any external dependency.

---

## Medium Range

### TLS for the admin port

The admin UI (port 8081) currently serves over plain HTTP. Fine on a private network, but not acceptable if admin access crosses a trust boundary. Plan: termination via a reverse proxy (Caddy) is already the recommended deployment pattern — native TLS support inside the binary is secondary.

### Pluggable secret backends

API keys currently come from environment variables. Extend the config to pull from:

- HashiCorp Vault
- Infisical
- Docker secrets / Kubernetes secrets

The env var path stays as the default; secret backends are opt-in.

### Rate limiting

Per-client and per-profile request limits. Token-bucket or sliding-window. Stored in memory — no Redis dependency.

### Response caching (semantic, opt-in)

For deterministic or near-deterministic prompts, cache the response against a hash of the request. Configurable TTL, configurable profiles. Reduces cost significantly for classification/routing agents that ask the same questions repeatedly.

---

## Vision

### Why not LiteLLM?

LiteLLM is a great project, but it carries a Python runtime, a PostgreSQL dependency (for production), and hundreds of megabytes of dependencies. That's the right tradeoff for teams that need its breadth.

lm-gateway-rs is for operators who want:

- A single static binary they can drop anywhere
- A config file under 50 lines
- Zero external state (no database, no cache server)
- Rust-level reliability and memory safety
- Transparent routing they can read and understand in an afternoon

The goal is not feature parity with LiteLLM. The goal is **the minimum surface required to run a reliable, cost-conscious AI routing layer** — and to do that so well that reaching for something heavier never makes sense for that use case.

### Where it's going

The routing intelligence gets smarter over time — semantic routing based on prompt content, automatic cost/quality optimization, learning which backends are reliable over time. But the deployment model stays the same: one binary, one config file, runs anywhere.

---

## Not In Scope

- A database (the traffic log is an in-memory ring buffer by design)
- A Python runtime or scripting layer
- A UI for configuring routing rules (config file is the interface)
- Autonomous model fine-tuning or training
