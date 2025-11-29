#!/usr/bin/env bash
set -euo pipefail

# Add WASM target if not present
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

# Build with release optimizations
echo "Building state-schema-conformance..."
cargo build -p state-schema-conformance --target wasm32-unknown-unknown --profile app-release

# Copy WASM file to res directory
mkdir -p res
cp target/wasm32-unknown-unknown/app-release/state_schema_conformance.wasm res/state-schema-conformance.wasm

# Optimize with wasm-opt if available
if command -v wasm-opt &> /dev/null; then
    echo "Optimizing WASM with wasm-opt..."
    wasm-opt -Os res/state-schema-conformance.wasm -o res/state-schema-conformance.wasm
fi

echo "Build complete: res/state-schema-conformance.wasm"

