#!/bin/bash
set -e

# Add wasm32 target if not already present
rustup target add wasm32-unknown-unknown || true

# Build the app
cargo build -p comprehensive-crdt-test --target wasm32-unknown-unknown --release

# Copy WASM file to res directory
mkdir -p res
cp target/wasm32-unknown-unknown/release/comprehensive_crdt_test.wasm res/comprehensive_crdt_test.wasm

# Optimize WASM if wasm-opt is available
if command -v wasm-opt &> /dev/null; then
    wasm-opt -O2 res/comprehensive_crdt_test.wasm -o res/comprehensive_crdt_test.wasm || true
fi

echo "Build complete: res/comprehensive_crdt_test.wasm"
