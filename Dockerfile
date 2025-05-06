# syntax=docker/dockerfile:1

# Comments are provided throughout this file to help you get started.
# If you need more help, visit the Dockerfile reference guide at
# https://docs.docker.com/go/dockerfile-reference/

# Want to help us make this template better? Share your feedback here: https://forms.gle/ybq9Krt8jtBL3iCk7

ARG RUST_VERSION=1.81.0
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
# Create a stage for building the Rust application
FROM rust:${RUST_VERSION}-slim AS builder-rust
WORKDIR /app

# Install system dependencies
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

# Copy Rust workspace files
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY contracts ./contracts
COPY apps ./apps
COPY e2e-tests ./e2e-tests

# Copy the built node-ui from the nodejs stage
COPY --from=builder-nodejs /app/node-ui ./node-ui

# Build merod and meroctl together with caching and copy to persistent locations
RUN --mount=type=cache,target=/app/target/ \
    --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo build --locked --release -p merod -p meroctl && \
    cp /app/target/release/merod /usr/local/bin/merod && \
    cp /app/target/release/meroctl /usr/local/bin/meroctl

################################################################################
# Create a minimal runner stage for merod
FROM debian:bookworm-slim AS merod

# Add labels for container metadata
LABEL org.opencontainers.image.description="Merod daemon"
LABEL org.opencontainers.image.licenses="MIT"

# Install only the essential runtime dependencies
RUN apt-get update && apt-get install -y \
    libssl3 \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create a non-privileged user for running the app
ARG UID=10001
RUN adduser \
    --disabled-password \
    --gecos "" \
    --home "/nonexistent" \
    --shell "/sbin/nologin" \
    --no-create-home \
    --uid "${UID}" \
    appuser

# Set the working directory
WORKDIR /data
RUN chown appuser:appuser /data

# Copy only the executable from the build stage
COPY --from=builder-rust /usr/local/bin/merod /usr/local/bin/merod

# Change to non-root user
USER appuser

# Set the entrypoint
ENTRYPOINT ["merod"]
CMD ["--help"]

################################################################################
# Create a minimal runner stage for meroctl
FROM debian:bookworm-slim AS meroctl

# Add labels for container metadata
LABEL org.opencontainers.image.description="Meroctl - Control tool for Merod daemon"
LABEL org.opencontainers.image.licenses="MIT"

# Install only the essential runtime dependencies
RUN apt-get update && apt-get install -y \
    libssl3 \
    ca-certificates \
    adduser \
    && rm -rf /var/lib/apt/lists/*

# Create a non-privileged user for running the app
ARG UID=10001
RUN adduser \
    --disabled-password \
    --gecos "" \
    --home "/nonexistent" \
    --shell "/sbin/nologin" \
    --no-create-home \
    --uid "${UID}" \
    appuser

# Set the working directory
WORKDIR /app
RUN chown appuser:appuser /app

# Copy only the executable from the build stage
COPY --from=builder-rust /usr/local/bin/meroctl /usr/local/bin/meroctl

# Change to non-root user
USER appuser

# Set the entrypoint
ENTRYPOINT ["meroctl"]
CMD ["--help"]
