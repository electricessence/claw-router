# Contributing to LM Gateway RS

Thanks for your interest. Here's what you need to know before contributing.

---

## Design principles

These are non-negotiable — contributions that violate them will not be accepted.

- **Single binary.** No runtime dependencies beyond libc/musl. Runs anywhere.
- **No Python.** The entire point of this project is to not be LiteLLM.
- **No database.** The traffic log is an in-memory ring buffer. No disk I/O required.
- **File size discipline.** Keep source files under ~500 lines. Split when approaching that.
- **Small surface.** Every feature must earn its place. Resist scope creep.

---

## Running locally

```sh
# Requires Rust 1.85+ (see rust-toolchain.toml)
cargo run -- --config config.example.toml
```

- Admin UI at `http://localhost:8081`
- Proxy endpoint at `http://localhost:8080`

---

## Building a Docker image

```sh
docker build --memory=3g --build-arg CARGO_BUILD_JOBS=2 -t lm-gateway .
```

By default this targets `amd64`. For multi-arch:

```sh
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  --build-arg CARGO_BUILD_JOBS=2 \
  -t lm-gateway --load .
```

---

## Making changes

1. Fork the repo and create a feature branch: `feature/<scope>`
2. Keep changes focused. One concern per PR.
3. Run `cargo check` and `cargo clippy` before pushing.
4. Every public item needs a doc comment.
5. Tests live in the same file as the code they test (Rust convention).

---

## Commit process

Follow the phased-commit procedure described in
[.github/instructions/phased-commit.instructions.md](.github/instructions/phased-commit.instructions.md):

1. Stage only files relevant to the current change
2. Review the diff critically
3. Check for secrets and PII — **never commit credentials, hostnames, or personal data**
4. Write a concise present-tense commit message
5. Push and open a PR

---

## Automated builds

Every push to `main` and every `v*.*.*` tag triggers the GitHub Actions workflow:

- Multi-arch image (`linux/amd64`, `linux/arm64`) published to GHCR
- SLSA build provenance attestation attached to the image
- Tagged releases also create a GitHub Release with auto-generated notes

---

## Questions

Open an issue. Keep it concise.
