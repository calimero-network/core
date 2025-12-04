# syntax=docker/dockerfile:1

# Dockerfile for prebuilt binaries

FROM ubuntu:24.04

LABEL org.opencontainers.image.description="Calimero Standalone Relayer Service" \
    org.opencontainers.image.licenses="MIT OR Apache-2.0" \
    org.opencontainers.image.authors="Calimero Limited <info@calimero.network>" \
    org.opencontainers.image.source="https://github.com/calimero-network/core" \
    org.opencontainers.image.url="https://calimero.network"

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    jq \
    curl \
    && rm -rf /var/lib/apt/lists/*

ARG UID=10001
RUN useradd \
    --home-dir "/user" \
    --create-home \
    --shell "/sbin/nologin" \
    --uid "${UID}" \
    user

ARG TARGETARCH

# Copy the prebuilt binary from the CI workflow artifacts
COPY \
    bin/${TARGETARCH}/mero-relayer \
    /usr/local/bin/

RUN chmod +x /usr/local/bin/mero-relayer

USER user
WORKDIR /data
ENV CALIMERO_HOME=/data

# Default relayer configuration
ENV RELAYER_PORT=63529
ENV RELAYER_LISTEN_URL="0.0.0.0:${RELAYER_PORT}"
ENV RUST_LOG="info"

# Enable/disable blockchain protocols
ENV ENABLE_NEAR=true
ENV ENABLE_STARKNET=false
ENV ENABLE_ICP=false
ENV ENABLE_ETHEREUM=false

# NEAR configuration
ENV NEAR_NETWORK="testnet"
ENV NEAR_RPC_URL="https://rpc.testnet.near.org"
ENV NEAR_CONTRACT_ID=""

# Starknet configuration  
ENV STARKNET_NETWORK="sepolia"
ENV STARKNET_RPC_URL="https://free-rpc.nethermind.io/sepolia-juno/"
ENV STARKNET_CONTRACT_ID=""

# ICP configuration
ENV ICP_NETWORK="local"
ENV ICP_RPC_URL="http://127.0.0.1:4943"
ENV ICP_CONTRACT_ID=""

# Ethereum configuration
ENV ETHEREUM_NETWORK="sepolia"
ENV ETHEREUM_RPC_URL="https://sepolia.drpc.org"
ENV ETHEREUM_CONTRACT_ID=""

# Default relayer URL for client configuration (can be overridden via environment)
ENV DEFAULT_RELAYER_URL=""

VOLUME /data
EXPOSE ${RELAYER_PORT}

# Health check
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:${RELAYER_PORT}/health || exit 1

ENTRYPOINT ["mero-relayer"]
CMD []
