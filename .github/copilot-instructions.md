# GitHub Copilot Instructions — lm-gateway-rs

> A minimal LLM routing gateway written in Rust. Single binary. No Python. No database. No bloat.

---

## ⚠️ No Secrets / No PII — Absolute Rule

**NEVER commit:**
- API keys, tokens, passwords, or any credentials
- Real hostnames, IP addresses, or internal server names
- Personal information (names, emails, phone numbers)
- SSH key paths or machine-specific paths

Use environment variables. Reference env var **names**, never **values**.

---

## ⚠️ No Infrastructure-Specific Tooling — Absolute Rule

**NEVER add to this repo:**
- Deploy scripts (`Push-ToLxc.ps1`, `Sync-*.ps1`, etc.)
- SSH wrappers or LXC management scripts
- Operational test runners that target a specific server
- `.env.ps1` / `.env.example.ps1` files with host aliases or container IDs
- Any file that assumes a particular server, LXC setup, or SSH topology

**Why:** This is a public, general-purpose project. Infrastructure-specific tooling leaks deployment context and does not belong here. It belongs in your private ops/infrastructure repo.

If you find yourself writing a script that contains `$SshAlias`, `$LxcId`, or any host-specific default — stop and put it in your private infrastructure repo instead.

---

## Project Identity

This is a **general-purpose LLM routing gateway**. It is not a Claw-specific product.

- Users are: developers, homelab operators, anyone who wants a lightweight LLM proxy
- Use case mentions like "AI agent clusters" or "homelab AI routing" are valid examples, not the primary framing
- The README and docs lead with the general value proposition: lightweight, single binary, works anywhere

Do not let Claw-specific framing creep back into comments, docs, or APIs.

---

## Public Repo Boundary — No Private Context

This is a **public repository**. Every file committed here is visible to anyone.

**Before adding any example, profile, script, or comment, ask: could this reveal anything about a private deployment?**

Specifically forbidden in any committed file:
- Names of private agents, services, or internal tools specific to your deployment
- References to private repos, internal docs, or deployment-specific infrastructure
- Example configs or profiles that are tailored to a specific private use case rather than being genuinely generic
- Security-sensitive profiles that are **deployment-specific** or purport to be authoritative (e.g. production access controllers, audit pipelines tied to a specific service) — these carry implied correctness guarantees and create liability if misused; keep them in private deployment configs. Generic illustrative examples are fine, provided they include a disclaimer that they are not production-ready.

**The `etc/lm-gateway/` directory is for generic, illustrative examples only.** Any profile that exists because of a specific private deployment need must stay in a private repo. If a profile is worth making generic and truly useful to any operator, strip all private context first and treat it as a new contribution on its own merits.

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

**Summary:** Stage → Critical Review → Security Audit → Commit → **await explicit push approval** → request fresh Copilot review if resolving PR comments.

---

## Config Deploy Discipline

Config files deployed to production (`etc/lm-gateway/config.toml`) must **always** originate from the repo. Never edit the live server config without the change being in the repo first.

- **Stage immediately** after any config change that will be deployed — even if not ready to commit yet. This prevents accidental loss during future syncs.
- **Deploy from repo** — `Sync-LmGateway.ps1` pushes repo files to LXC. The repo is the source of truth.
- **Profile deletion = explicit intent** — removing a profile section from the config requires a clear justification (not an accidental side-effect of a large rewrite).
