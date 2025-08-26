# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.88.0
# ^~~ keep this in sync with rust-toolchain.toml

################################################################################
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
COPY e2e-tests ./e2e-tests

ARG CALIMERO_WEBUI_SRC # the url or absolute path to the webui (archive or directory)
ARG CALIMERO_WEBUI_REPO # the git repository hosting the webui (for a git release asset)
ARG CALIMERO_WEBUI_VERSION # the version of the webui to use (for a git release asset)
ARG CALIMERO_WEBUI_FETCH # invalidate the cache, fetch the webui (for a git release asset)
ARG CALIMERO_WEBUI_ASSET # file name of the asset to use (for a git release asset)
# CALIMERO_WEBUI_FETCH_TOKEN # GitHub token to use for fetching the webui (for a git release asset)

# ^~~ docker build
#        --build-arg CALIMERO_WEBUI_FETCH=1
#   env: --secret id=gh-token,env=CALIMERO_WEBUI_FETCH_TOKEN
#  file: --secret id=gh-token,src=./gh_token.txt

RUN --mount=type=cache,target=/app/target/ \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    --mount=type=secret,id=gh-token,env=CALIMERO_WEBUI_FETCH_TOKEN \
    [ -n "$CALIMERO_WEBUI_FETCH_TOKEN" ] || unset CALIMERO_WEBUI_FETCH_TOKEN && \
    cargo build --locked --release -p merod -p meroctl && \
    cp /app/target/release/merod /app/target/release/meroctl /usr/local/bin/

################################################################################
FROM debian:bookworm-slim AS runtime

LABEL org.opencontainers.image.description="Calimero Node" \
    org.opencontainers.image.licenses="MIT OR Apache-2.0" \
    org.opencontainers.image.authors="Calimero Limited <info@calimero.network>" \
    org.opencontainers.image.source="https://github.com/calimero-network/core" \
    org.opencontainers.image.url="https://calimero.network"

RUN apt-get update && apt-get install -y --no-install-recommends \
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

COPY --from=build \
    /usr/local/bin/merod \
    /usr/local/bin/meroctl \
    /usr/local/bin/

USER user
WORKDIR /data
ENV CALIMERO_HOME=/data

VOLUME /data
EXPOSE 2428 2528

ENTRYPOINT ["merod"]
CMD ["--help"]
