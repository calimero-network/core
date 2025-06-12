# syntax=docker/dockerfile:1

# Dockerfile for prebuilt binaries

FROM debian:bookworm-slim

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

ARG TARGETARCH
ARG BINARY_NAME

# Copy the prebuilt binary from the CI workflow artifacts
COPY \
    bin/${TARGETARCH}/merod \
    bin/${TARGETARCH}/meroctl \
    .github/workflows/deps/entrypoint.sh \
    /usr/local/bin/

RUN chmod +x /usr/local/bin/{merod,meroctl}

USER user
WORKDIR /data
ENV CALIMERO_HOME=/data

ENTRYPOINT ["/usr/local/bin/merod"]
CMD ["--help"]
