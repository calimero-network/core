# syntax=docker/dockerfile:1

# Dockerfile for prebuilt binaries

FROM ubuntu:24.04

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

# Create app directory and set permissions
RUN mkdir -p /app && chown appuser:appuser /app

# Change to non-root user
USER appuser
WORKDIR /app

# Set the entrypoint using the binary name
ENTRYPOINT ["/usr/local/bin/${BINARY_NAME}"]
CMD ["--help"]
