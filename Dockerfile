# claw-router — multi-stage Rust build
# Apply mold linker fix per docs/gotchas.md (OOM + speed)
#
# Build args:
#   CARGO_BUILD_JOBS=2    — cap parallelism for low-RAM hosts (5.8 GB)
#
# Usage:
#   docker build --memory=3g --build-arg CARGO_BUILD_JOBS=2 -t claw-router .

FROM rust:1.82-slim-bookworm AS builder

ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}

# System deps: mold linker (fast + low-RAM), clang (for mold integration), SSL certs
RUN apt-get update && apt-get install -y --no-install-recommends \
    mold \
    clang \
    ca-certificates \
    libssl-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Configure mold as the linker to reduce peak RAM during link step
RUN mkdir -p .cargo && printf '[target.x86_64-unknown-linux-gnu]\nlinker = "clang"\nrustflags = ["-C", "link-arg=-fuse-ld=mold"]\n' > .cargo/config.toml

# Cache dependencies separately from source
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --jobs ${CARGO_BUILD_JOBS} 2>&1 \
    && rm -rf src target/release/.fingerprint/claw-router-*

# Build real source
COPY src ./src
RUN cargo build --release --jobs ${CARGO_BUILD_JOBS}

# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    wget \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -r -s /bin/false claw
USER claw

COPY --from=builder /build/target/release/claw-router /usr/local/bin/claw-router

ENV CLAW_ROUTER_CONFIG=/etc/claw-router/config.toml

EXPOSE 8080 8081

HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
    CMD wget -qO- http://localhost:8080/healthz > /dev/null || exit 1

ENTRYPOINT ["/usr/local/bin/claw-router"]
