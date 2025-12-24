# syntax=docker/dockerfile:1

# Dockerfile for prebuilt binaries with profiling tools
# This image includes perf, flamegraph, jemalloc, and heaptrack for performance analysis

# Build stage: Compile binaries with frame pointers enabled
ARG RUST_VERSION=1.88.0
FROM rust:${RUST_VERSION}-slim-bookworm AS build

RUN apt-get update && apt-get install -y --no-install-recommends \
    clang \
    make \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps ./apps
COPY tools ./tools

ARG CALIMERO_WEBUI_SRC
ARG CALIMERO_WEBUI_REPO
ARG CALIMERO_WEBUI_VERSION
ARG CALIMERO_WEBUI_FETCH
ARG CALIMERO_WEBUI_ASSET

# Build with frame pointers enabled 
ENV RUSTFLAGS="-C force-frame-pointers=yes"

RUN --mount=type=cache,target=/app/target/ \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    --mount=type=secret,id=gh-token,env=CALIMERO_WEBUI_FETCH_TOKEN \
    [ -n "$CALIMERO_WEBUI_FETCH_TOKEN" ] || unset CALIMERO_WEBUI_FETCH_TOKEN && \
    echo "Building merod and meroctl with frame pointers enabled..." && \
    cargo build --locked --release -p merod -p meroctl && \
    cp /app/target/release/merod /app/target/release/meroctl /usr/local/bin/

# Runtime stage: Profiling tools and binaries
FROM ubuntu:24.04

LABEL org.opencontainers.image.description="Calimero Node with Profiling Tools" \
    org.opencontainers.image.licenses="MIT OR Apache-2.0" \
    org.opencontainers.image.authors="Calimero Limited <info@calimero.network>" \
    org.opencontainers.image.source="https://github.com/calimero-network/core" \
    org.opencontainers.image.url="https://calimero.network"

# Install base dependencies and profiling tools
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    # Profiling tools
    linux-tools-generic \
    linux-tools-common \
    # Memory profiling
    heaptrack \
    # System monitoring
    htop \
    procps \
    # Build tools for flamegraph and jemalloc
    git \
    curl \
    perl \
    # jemalloc dependencies
    build-essential \
    autoconf \
    # Python for additional processing
    python3 \
    python3-pip \
    # Utilities
    gzip \
    tar \
    && rm -rf /var/lib/apt/lists/*

# Build jemalloc from source
ARG JEMALLOC_VERSION=5.3.0
RUN curl -fsSL "https://github.com/jemalloc/jemalloc/releases/download/${JEMALLOC_VERSION}/jemalloc-${JEMALLOC_VERSION}.tar.bz2" \
        -o /tmp/jemalloc.tar.bz2 \
    && cd /tmp \
    && tar -xjf jemalloc.tar.bz2 \
    && cd "jemalloc-${JEMALLOC_VERSION}" \
    && ./configure --enable-prof --prefix=/usr/local \
    && make -j$(nproc) \
    && make install \
    && ldconfig \
    && rm -rf /tmp/jemalloc* \
    && echo "[jemalloc] Built with profiling support"

# Install FlameGraph tools
RUN git clone --depth 1 https://github.com/brendangregg/FlameGraph.git /opt/FlameGraph \
    && chmod +x /opt/FlameGraph/*.pl

# Create symlinks for flamegraph tools
RUN ln -s /opt/FlameGraph/stackcollapse-perf.pl /usr/local/bin/stackcollapse-perf.pl \
    && ln -s /opt/FlameGraph/flamegraph.pl /usr/local/bin/flamegraph.pl \
    && ln -s /opt/FlameGraph/stackcollapse.pl /usr/local/bin/stackcollapse.pl \
    && ln -s /opt/FlameGraph/difffolded.pl /usr/local/bin/difffolded.pl

# Create profiling directories
RUN mkdir -p /profiling/data /profiling/reports /profiling/scripts

# Copy profiling scripts
COPY scripts/profiling/ /profiling/scripts/
RUN chmod +x /profiling/scripts/*.sh

ARG UID=10001
RUN useradd \
    --home-dir "/user" \
    --create-home \
    --shell "/bin/bash" \
    --uid "${UID}" \
    user

# Give user access to profiling directories
RUN chown -R user:user /profiling

# Copy binaries from build stage (compiled with frame pointers)
COPY --from=build /usr/local/bin/merod /usr/local/bin/meroctl /usr/local/bin/

RUN chmod +x /usr/local/bin/merod /usr/local/bin/meroctl

# Environment variables for profiling
# jemalloc profiling configuration
ENV MALLOC_CONF="prof:true,prof_prefix:/profiling/data/jemalloc,lg_prof_interval:30,prof_gdump:true,prof_final:true"
# Enable jemalloc as the allocator
# Using source-built jemalloc at /usr/local/lib
ENV LD_PRELOAD_JEMALLOC="/usr/local/lib/libjemalloc.so.2"
# Profiling output directory
ENV PROFILING_OUTPUT_DIR="/profiling/data"
ENV PROFILING_REPORTS_DIR="/profiling/reports"
# FlameGraph location
ENV FLAMEGRAPH_DIR="/opt/FlameGraph"
# Default perf sample frequency (samples per second)
ENV PERF_SAMPLE_FREQ="99"
# Enable debug symbols (useful for profiling)
ENV RUST_BACKTRACE="1"

# Working directory for data
WORKDIR /data
ENV CALIMERO_HOME=/data

VOLUME /data
VOLUME /profiling

EXPOSE 2428 2528

# Use a wrapper entrypoint that can optionally enable profiling
COPY scripts/profiling/entrypoint-profiling.sh /usr/local/bin/entrypoint-profiling.sh
RUN chmod +x /usr/local/bin/entrypoint-profiling.sh

# Run as root to allow perf access (can be changed at runtime)
# For perf to work, the container needs either:
# - CAP_SYS_ADMIN capability
# - --privileged flag
# - kernel.perf_event_paranoid set to -1 or 0 on the host

ENTRYPOINT ["/usr/local/bin/entrypoint-profiling.sh"]
CMD ["merod", "--help"]

