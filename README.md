# claw-router

A lightweight, opinionated LLM routing gateway written in Rust, built for the ZeroClaw ecosystem.

## What it does

claw-router sits between ZeroClaw agents and LLM backends, intelligently routing requests across a tier ladder — from fast/cheap local models up to cloud expert models — based on configurable profiles.

```
ZeroClaw agent  →  claw-router :8080  →  Ollama (local)
                                      →  OpenRouter (cloud)
                                      →  Any OpenAI-compatible API
```

## Routing modes

| Mode | Strategy | Latency | Cost |
|------|----------|---------|------|
| **Dispatch** | Pre-classify intent with a fast model, route to the right tier immediately | +200–800ms | Predictable |
| **Escalate** | Try cheapest tier first; evaluate response; escalate if insufficient | Lower average | Variable |

## Default tier ladder

```
local:fast  →  local:capable  →  cloud:economy  →  cloud:standard  →  cloud:expert
```

Each tier maps to a backend+model pair. Tiers are fully configurable.

## Quick start

```bash
# Copy and edit the example config
cp config.example.toml config.toml
$EDITOR config.toml

# Set secrets via environment variables (never in config file)
export OPENROUTER_KEY="sk-or-..."

# Run (Docker)
docker run --rm \
  -v $(pwd)/config.toml:/etc/claw-router/config.toml:ro \
  -e OPENROUTER_KEY \
  -p 8080:8080 -p 8081:8081 \
  claw-router:latest
```

## API

### Client API (port 8080) — ZeroClaw-compatible

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/v1/chat/completions` | Route a chat request |
| `GET`  | `/v1/models` | List available tiers and aliases |
| `GET`  | `/healthz` | Liveness probe |

Use any tier name or alias as the `model` field:

```json
{
  "model": "hint:fast",
  "messages": [{ "role": "user", "content": "Hello" }]
}
```

### Admin API (port 8081)

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/admin/health` | Gateway health + tier/backend counts |
| `GET` | `/admin/traffic?limit=N` | Recent N requests + aggregate stats |
| `GET` | `/admin/config` | Current config (secrets redacted) |
| `GET` | `/admin/backends/health` | Probe all configured backends |

## Configuration

See [config.example.toml](config.example.toml) for a fully annotated example.

Key concepts:
- **Backends** — named LLM providers with a base URL and optional secret env var
- **Tiers** — named (backend, model) pairs in cheapest→best order
- **Aliases** — short names like `hint:fast` that resolve to a tier
- **Profiles** — routing behaviour (mode, classifier tier, cost ceiling, expert gate)

## ZeroClaw integration

In your ZeroClaw `config.toml`, point the session's model route at claw-router:

```toml
[[model_routes]]
match = { sender_id = 123456789 }
provider = "openai_compatible"
base_url = "http://claw-router:8080/v1"
model = "hint:fast"   # or any alias/tier name
```

## Building

```bash
# Development
cargo build

# Production (cap RAM for low-memory hosts)
docker build --memory=3g --build-arg CARGO_BUILD_JOBS=2 -t claw-router .
```

## Project layout

```
claw-router/
├── Cargo.toml
├── Dockerfile
├── config.example.toml
└── src/
    ├── main.rs          # Startup, dual listeners, shutdown
    ├── config.rs        # Config types, TOML loading, validation
    ├── router.rs        # Routing logic (dispatch + escalate)
    ├── traffic.rs       # In-memory traffic ring buffer
    ├── backends/
    │   └── mod.rs       # HTTP client wrapper for LLM backends
    └── api/
        ├── mod.rs
        ├── health.rs    # GET /healthz
        ├── client.rs    # POST /v1/chat/completions, GET /v1/models
        └── admin.rs     # Admin endpoints
```

## Status

Early scaffold — not yet production-ready. Core routing logic and API layer are functional; streaming support, web UI, and hot-reload are planned.
