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
    bin/${TARGETARCH}/mero-kms-phala \
    /usr/local/bin/

RUN chmod +x /usr/local/bin/mero-kms-phala /usr/local/bin/mero-kms-phala

USER user

ENV LISTEN_ADDR=0.0.0.0:8080
ENV DSTACK_SOCKET_PATH=/var/run/dstack.sock
ENV ACCEPT_MOCK_ATTESTATION=false
ENV RUST_LOG=info

VOLUME /data
EXPOSE 8080

CMD ["mero-kms-phala"]
