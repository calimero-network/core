# syntax=docker/dockerfile:1

# Dockerfile for prebuilt binaries stages used in CI

################################################################################
# Base image for prebuilt binaries
FROM ubuntu:24.04 AS prebuilt-base

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

# Set architecture-specific path for multi-platform builds
ARG TARGETARCH
ARG BINARY_NAME

# Copy the prebuilt binary from the CI workflow artifacts
WORKDIR /
COPY bin/${TARGETARCH}/${BINARY_NAME} /usr/local/bin/${BINARY_NAME}
RUN chmod +x /usr/local/bin/${BINARY_NAME}

# Change to non-root user
USER appuser

################################################################################
# Create a generic runner stage using prebuilt binaries
FROM prebuilt-base AS generic-prebuilt

# Set the working directory based on binary name
ARG BINARY_NAME
RUN if [ "$BINARY_NAME" = "merod" ]; then \
      mkdir -p /data && \
      chown appuser:appuser /data && \
      echo "/data" > /tmp/workdir; \
    else \
      mkdir -p /app && \
      chown appuser:appuser /app && \
      echo "/app" > /tmp/workdir; \
    fi
WORKDIR /placeholder
RUN WORKDIR=$(cat /tmp/workdir) && rm /tmp/workdir && cd $WORKDIR

# Set the entrypoint using the binary name
ENTRYPOINT ["/usr/local/bin/${BINARY_NAME}"]
CMD ["--help"]

################################################################################
# Create aliased targets for backward compatibility
FROM generic-prebuilt AS merod-prebuilt
FROM generic-prebuilt AS meroctl-prebuilt