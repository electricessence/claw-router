# lm-gateway-rs — multi-stage Rust build
#
# Builder: alpine:3 + rustup — fully controlled, patchable base.
#          Compiles to a fully static musl binary via the appropriate target.
#          reqwest uses rustls — no OpenSSL, no C TLS dependency.
#
# Runtime: scratch — empty image, zero CVEs, nothing to scan.
#          Only the static binary + CA certs + a passwd entry are present.
#
# Build args:
#   CARGO_BUILD_JOBS=2    — cap parallelism for low-RAM hosts
#   TARGETARCH            — set automatically by `docker buildx` (amd64 / arm64)
#
# Usage (single-arch):
#   docker build --memory=3g --build-arg CARGO_BUILD_JOBS=2 -t lm-gateway .
#
# Usage (multi-arch via buildx):
#   docker buildx build --platform linux/amd64,linux/arm64 \
#     --memory=3g --build-arg CARGO_BUILD_JOBS=2 \
#     -t ghcr.io/<owner>/lm-gateway-rs:latest --push .

FROM alpine:3 AS builder

ARG CARGO_BUILD_JOBS=2
ARG TARGETARCH=amd64

# Map Docker platform arch → Rust musl target triple
RUN case "${TARGETARCH}" in \
      amd64) echo "x86_64-unknown-linux-musl"  > /rust_target ;; \
      arm64) echo "aarch64-unknown-linux-musl" > /rust_target ;; \
      *) echo "Unsupported TARGETARCH: ${TARGETARCH}" && exit 1 ;; \
    esac

# Patch all Alpine packages, then add build deps.
# musl-dev: static linking target
# curl: rustup installer
RUN apk upgrade --no-cache \
    && apk add --no-cache curl musl-dev

# Install Rust via rustup (pinned version, target determined above)
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN RUST_TARGET=$(cat /rust_target) \
    && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
       | sh -s -- -y --no-modify-path --profile minimal \
           --default-toolchain 1.85.0 \
           --target "${RUST_TARGET}"

WORKDIR /build

# Cache dependencies separately from source
COPY Cargo.toml Cargo.lock ./
RUN RUST_TARGET=$(cat /rust_target) \
    && mkdir -p src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --jobs ${CARGO_BUILD_JOBS} \
        --no-default-features --features rustls-tls \
        --target "${RUST_TARGET}" \
    && rm -rf src "target/${RUST_TARGET}/release/.fingerprint/lm-gateway-"*

# Build real source
COPY src ./src
RUN RUST_TARGET=$(cat /rust_target) \
    && cargo build --release --jobs ${CARGO_BUILD_JOBS} \
        --no-default-features --features rustls-tls \
        --target "${RUST_TARGET}" \
    && cp "target/${RUST_TARGET}/release/lm-gateway" /lm-gateway

# Minimal passwd file so the binary can run as a named non-root user
RUN echo "gateway:x:65534:65534:gateway:/:/sbin/nologin" > /tmp/passwd

# ---------------------------------------------------------------------------
FROM scratch AS runtime

# CA certificates — required for outbound HTTPS to LLM backends
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

# Non-root user identity (no shell, no home)
COPY --from=builder /tmp/passwd /etc/passwd

COPY --from=builder /lm-gateway /lm-gateway

USER gateway

ENV LMG_CONFIG=/etc/lm-gateway/config.toml

EXPOSE 8080 8081

# scratch has no shell or wget — use the binary's own healthz endpoint via HTTP
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD ["/lm-gateway", "--healthcheck"]

ENTRYPOINT ["/lm-gateway"]
