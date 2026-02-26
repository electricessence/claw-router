# lm-gateway-rs — multi-stage Rust build
#
# Builder: alpine:3 + rustup — fully controlled, patchable base.
#          Compiles to a fully static musl binary (x86_64-unknown-linux-musl).
#          reqwest uses rustls — no OpenSSL, no C TLS dependency.
#
# Runtime: scratch — empty image, zero CVEs, nothing to scan.
#          Only the static binary + CA certs + a passwd entry are present.
#
# Build args:
#   CARGO_BUILD_JOBS=2    — cap parallelism for low-RAM hosts
#
# Usage:
#   docker build --memory=3g --build-arg CARGO_BUILD_JOBS=2 -t lm-gateway .

FROM alpine:3 AS builder

ARG CARGO_BUILD_JOBS=2

# Patch all Alpine packages, then add build deps.
# musl-dev: static linking target
# curl: rustup installer
# gcc: C compiler for ring and other crates with C code
RUN apk upgrade --no-cache \
    && apk add --no-cache curl musl-dev gcc

# Install Rust via rustup.
# CC_x86_64_unknown_linux_musl: Alpine's native gcc IS the musl compiler.
# ring/cc-rs looks for "x86_64-linux-musl-gcc" by name — this redirects it.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH \
    CC_x86_64_unknown_linux_musl=gcc
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
       | sh -s -- -y --no-modify-path --profile minimal \
           --default-toolchain 1.85.0 \
           --target x86_64-unknown-linux-musl

WORKDIR /build

# Cache dependencies separately from source
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --jobs ${CARGO_BUILD_JOBS} \
        --no-default-features --features rustls-tls \
        --target x86_64-unknown-linux-musl \
    && rm -rf src target/x86_64-unknown-linux-musl/release/.fingerprint/lm-gateway-*

# Build real source
COPY src ./src
RUN cargo build --release --jobs ${CARGO_BUILD_JOBS} \
        --no-default-features --features rustls-tls \
        --target x86_64-unknown-linux-musl \
    && cp target/x86_64-unknown-linux-musl/release/lm-gateway /lm-gateway

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
