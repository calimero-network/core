# syntax=docker/dockerfile:1

# Dockerfile for prebuilt binaries

FROM ubuntu:24.04

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

ARG TARGETARCH

# Copy the prebuilt binary from the CI workflow artifacts
COPY \
    bin/${TARGETARCH}/merod \
    bin/${TARGETARCH}/meroctl \
    /usr/local/bin/

RUN chmod +x /usr/local/bin/merod /usr/local/bin/meroctl

USER user
WORKDIR /data
ENV CALIMERO_HOME=/data

VOLUME /data
EXPOSE 2428 2528

ENTRYPOINT ["merod"]
CMD ["--help"]
