# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.85.0
# ^~~ keep this in sync with rust-toolchain.toml

ARG NODE_VERSION=20

################################################################################
# Create a stage for building the node-ui application
FROM node:${NODE_VERSION}-slim AS builder-nodejs
WORKDIR /app

# Copy node-ui directory
COPY node-ui ./node-ui
WORKDIR /app/node-ui

# Install pnpm and build the UI
RUN npm install -g pnpm && \
    pnpm install --no-frozen-lockfile && \
    pnpm run build

################################################################################
FROM rust:${RUST_VERSION}-slim AS builder-rust

RUN apt-get update && apt-get install -y \
    clang \
    libclang-dev \
    cmake \
    git \
    pkg-config \
    libssl-dev \
    zlib1g-dev \
    libsnappy-dev \
    libbz2-dev \
    liblz4-dev \
    libzstd-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps ./apps
COPY e2e-tests ./e2e-tests

COPY --from=builder-nodejs /app/node-ui ./node-ui

RUN --mount=type=cache,target=/app/target/ \
    --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo build --locked --release -p merod -p meroctl && \
    cp /app/target/release/merod /usr/local/bin/merod && \
    cp /app/target/release/meroctl /usr/local/bin/meroctl

################################################################################
FROM debian:bookworm-slim as runtime

LABEL org.opencontainers.image.description="Calimero Node" \
    org.opencontainers.image.licenses="MIT OR Apache-2.0" \
    org.opencontainers.image.authors="Calimero Limited <info@calimero.network>" \
    org.opencontainers.image.source="https://github.com/calimero-network/core" \
    org.opencontainers.image.url="https://calimero.network"

RUN apt-get update && apt-get install -y \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ARG UID=10001
RUN adduser \
    --disabled-password \
    --gecos "" \
    --home "/user" \
    --shell "/sbin/nologin" \
    --uid "${UID}" \
    user

COPY --from=builder-rust /usr/local/bin/merod /usr/local/bin/merod
COPY --from=builder-rust /usr/local/bin/meroctl /usr/local/bin/meroctl

USER user
WORKDIR /data
ENV CALIMERO_HOME=/data

ENTRYPOINT ["merod"]
CMD ["--help"]
