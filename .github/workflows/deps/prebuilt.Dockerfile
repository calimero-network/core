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

# Copy the prebuilt binaries from the CI workflow artifacts
COPY \
    bin/${TARGETARCH}/merod \
    bin/${TARGETARCH}/meroctl \
    /usr/local/bin/

RUN chmod +x /usr/local/bin/merod

COPY ./scripts/container/start-merod.sh /usr/local/bin/start.sh
RUN chmod +x /usr/local/bin/start.sh

# Confidential Space Launch Policy Configuration
# Reference: https://docs.cloud.google.com/confidential-computing/confidential-space/docs/reference/launch-policies
LABEL "tee.launch_policy.allow_cmd_override"="true"
LABEL "tee.launch_policy.allow_env_override"="CALIMERO_HOME,NODE_NAME,RUST_LOG,RUST_BACKTRACE,NO_COLOR"
LABEL "tee.launch_policy.log_redirect"="always"
LABEL "tee.launch_policy.allow_mount_destinations"="/data"
LABEL "tee.launch_policy.monitoring_memory_allow"="always"
LABEL "tee.launch_policy.allow_capabilities"="false"
LABEL "tee.launch_policy.allow_cgroups"="false"

USER user
WORKDIR /data
ENV CALIMERO_HOME=/data

VOLUME /data
EXPOSE 2428 2528

CMD ["/bin/sh", "/usr/local/bin/start.sh"]
