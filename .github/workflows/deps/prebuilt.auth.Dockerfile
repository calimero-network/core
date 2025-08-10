# syntax=docker/dockerfile:1

# Dockerfile for prebuilt binaries

FROM ubuntu:24.04

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
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
    bin/${TARGETARCH}/mero-auth \
    /usr/local/bin/
COPY crates/auth/config/config.toml /etc/calimero/auth.toml

RUN chmod +x /usr/local/bin/mero-auth

USER user
WORKDIR /data

VOLUME /data
EXPOSE 3001

ENTRYPOINT mero-auth --config /etc/calimero/auth.toml --verbose
