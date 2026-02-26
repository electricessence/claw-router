# GitHub Copilot Instructions — lm-gateway-rs

> A minimal LLM routing gateway written in Rust. Single binary. No Python. No database. No bloat.

---

## ⚠️ No Secrets / No PII

**NEVER commit:**
- API keys, tokens, passwords, or any credentials
- Real hostnames, IP addresses, or internal server names
- Personal information (names, emails, phone numbers)
- SSH key paths or machine-specific paths

Use environment variables. Reference env var **names**, never **values**.

---

## Project Identity

This is a **general-purpose LLM routing gateway**. It is not a Claw-specific product.

- Users are: developers, homelab operators, anyone who wants a lightweight LLM proxy
- Use case mentions like "AI agent clusters" or "ZeroClaw" are valid examples, not the primary framing
- The README and docs lead with the general value proposition: lightweight, single binary, works anywhere

Do not let Claw-specific framing creep back into comments, docs, or APIs.

---

## Design Principles (Non-Negotiable)

1. **Single binary** — no runtime dependencies beyond libc. Runs anywhere.
2. **No Python** — forbidden. The entire point of this project is to not be LiteLLM.
3. **No database** — the traffic log is an in-memory ring buffer. No disk I/O required.
4. **File size discipline** — keep source files under ~500 lines. Split when approaching that limit.
5. **Small surface** — every feature must earn its place. Resist scope creep.
6. **Transparent config** — TOML, under 50 lines for a full production setup.

---

## Code Standards

- Idiomatic Rust — `anyhow` for errors, `tracing` for observability, `tokio` for async
- Every public item has a doc comment
- Tests live in the same file as the code they test (Rust convention)
- Follow the existing patterns in `router.rs`, `traffic.rs`, `config.rs`

---

## Build Constraints

Docker builds for Rust on low-RAM hosts:
```
docker build --memory=3g --build-arg CARGO_BUILD_JOBS=2 -t lm-gateway .
```

Never exceed 2 parallel Cargo jobs in Docker. The host has limited RAM.

---

## Commit & Push Procedure

See `.github/instructions/phased-commit.instructions.md` for the full procedure.

**Summary:** Stage → Critical Review → Security Audit → Commit → **await explicit push approval**.
