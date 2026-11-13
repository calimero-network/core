# syntax=docker/dockerfile:1

# Dockerfile for prebuilt binaries

FROM ubuntu:24.04@sha256:4fbb8e6a8395de5a7550b33509421a2bafbc0aab6c06ba2cef9ebffbc7092d90

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

# Copy the prebuilt binary from the CI workflow artifacts. The image runs a node
# and nothing else: meroctl and the other tools ship as release archives and
# through Homebrew, and talk to the node from the host.
COPY bin/${TARGETARCH}/merod /usr/local/bin/

RUN chmod +x /usr/local/bin/merod

USER user
WORKDIR /data
ENV CALIMERO_HOME=/data
# `merod --node NAME init ...` then `merod --node NAME run`, or `merod run` with
# MEROD_* variables only. For development, a three-node cluster reachable from the
# host: `docker run -p 2501-2503:2501-2503 -v merod-local:/data <image> local --bind 0.0.0.0`.

VOLUME /data
# The API (sealed channel, `/attestation`, `/health`); peers use it too.
EXPOSE 2528
# `merod local`'s three nodes.
EXPOSE 2501-2503

ENTRYPOINT ["merod"]
CMD ["--help"]
