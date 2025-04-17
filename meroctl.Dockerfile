# Use an official Rust image as the base
FROM rust:latest as builder

# Set environment variables
ENV CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    PATH=/usr/local/cargo/bin:$PATH

# Install system dependencies
RUN apt-get update && apt-get install -y \
    zlib1g-dev \
    libsnappy-dev \
    libbz2-dev \
    liblz4-dev \
    libzstd-dev \
    clang \
    libclang-dev \
    curl \
    build-essential \
    pkg-config \
    jq \
    && rm -rf /var/lib/apt/lists/*

# Set the working directory
WORKDIR /app

# Copy all workspace members
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY contracts ./contracts
COPY apps ./apps
COPY node-ui ./node-ui
COPY e2e-tests ./e2e-tests

# Build the meroctl binary
RUN cargo build --release -p meroctl

# Copy the binary to a location in PATH
RUN cp /app/target/release/meroctl /usr/local/bin/meroctl

# Default command (can be overridden in docker-compose)
CMD ["--help"] 