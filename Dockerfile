# Use an official Rust image as the base
FROM rust:latest

# Install system dependencies
RUN  apt-get update && apt-get install -y \
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
    jq

# Install Node.js (version 20) and pnpm
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - && \
    apt-get install -y nodejs && \
    npm install -g pnpm

# Set the working directory
WORKDIR /app

# Copy only necessary files for building dependencies
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY contracts ./contracts
COPY apps ./apps
COPY node-ui ./node-ui
COPY e2e-tests ./e2e-tests

# Build the node UI
WORKDIR /app/node-ui
RUN pnpm install && pnpm run build

# Build the merod binary
WORKDIR /app
RUN cargo build --release -p merod

# Set the binary as the entrypoint
ENTRYPOINT ["/app/target/release/merod"]

# Default command (can be overridden in docker-compose)
CMD ["--help"]
