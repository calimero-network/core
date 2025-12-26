#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname $0)"

# Add WASM target if not present
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

TARGET="${CARGO_TARGET_DIR:-../../target}"

# Build with release optimizations
echo "Building state-schema-conformance..."
cargo build -p state-schema-conformance --target wasm32-unknown-unknown --profile app-release

# Copy WASM file to res directory
mkdir -p res
cp $TARGET/wasm32-unknown-unknown/app-release/state_schema_conformance.wasm ./res/

# Optimize with wasm-opt if available
if command -v wasm-opt &> /dev/null; then
    echo "Optimizing WASM with wasm-opt..."
    wasm-opt -Os res/state_schema_conformance.wasm -o ./res/state_schema_conformance.wasm
fi

echo "Build complete: res/state_schema_conformance.wasm"

