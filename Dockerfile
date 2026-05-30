# syntax=docker/dockerfile:1
#
# Single Rust image: builds the Svelte SPA, embeds it (via rust-embed at
# compile time), then builds ALL Rust binaries (gateway, ingest, regime,
# router, risk, goalpost, devpub) into one distroless image. Each k8s pod
# picks its own entrypoint with `command:` — one pull per node, one SBOM.

FROM node:26-alpine AS web
WORKDIR /app/web
COPY web/package.json web/package-lock.json web/.npmrc ./
RUN npm ci --ignore-scripts
COPY web/ ./
RUN npm run build            # → /app/web/dist (vite outDir, embedded by rust-embed)

FROM rust:1.95-slim-bookworm AS build
ENV CARGO_TERM_COLOR=always CARGO_HOME=/usr/local/cargo
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app

# Cache deps in their own layer: copy manifest first, build a stub.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src/bin && \
    echo "fn main() {}" > src/bin/_warm.rs && \
    echo "pub fn _w() {}" > src/lib.rs && \
    echo '[[bin]]\nname = "_warm"\npath = "src/bin/_warm.rs"\n' >> Cargo.toml && \
    cargo build --release --bin _warm || true && \
    rm -rf src

# Now bring in the real source, including the SPA dist for rust-embed.
COPY . .
COPY --from=web /app/web/dist ./web/dist
RUN cargo build --release --bins

# Distroless runtime: cc-debian12 has libgcc_s / glibc which our binaries need
# (sqlx-rustls + reqwest don't need OpenSSL but the dynamic loader's there).
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /app/target/release/gateway \
                  /app/target/release/ingest \
                  /app/target/release/regime \
                  /app/target/release/router \
                  /app/target/release/risk \
                  /app/target/release/goalpost \
                  /app/target/release/devpub /
USER nonroot
EXPOSE 8080
# No ENTRYPOINT — set `command:` per pod, e.g. ["/gateway"] or ["/ingest"].
