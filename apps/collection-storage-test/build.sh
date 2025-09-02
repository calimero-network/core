#!/bin/bash

echo "🔨 Building Collection Storage Test Application..."

# Build the application
cargo build --target wasm32-unknown-unknown --release

# Create res directory if it doesn't exist
mkdir -p res

# Copy the built WASM to res directory (from workspace root)
cp ../../target/wasm32-unknown-unknown/release/collection_storage_test.wasm res/

echo "✅ Build complete! WASM file: res/collection_storage_test.wasm"
echo "📊 File size: $(du -h res/collection_storage_test.wasm | cut -f1)"
