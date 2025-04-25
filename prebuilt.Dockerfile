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
# Create a minimal runner stage for merod using prebuilt binaries
FROM prebuilt-base AS merod-prebuilt

# Set the working directory
WORKDIR /data
RUN chown appuser:appuser /data

# Set the entrypoint
ENTRYPOINT ["merod"]
CMD ["--help"]

################################################################################
# Create a minimal runner stage for meroctl using prebuilt binaries
FROM prebuilt-base AS meroctl-prebuilt

# Set the working directory
WORKDIR /app
RUN chown appuser:appuser /app

# Set the entrypoint
ENTRYPOINT ["meroctl"]
CMD ["--help"]