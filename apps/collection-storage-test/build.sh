#!/bin/bash

set -e  # Exit on any error

echo "🔨 Building Collection Storage Test Application..."

# Check if WASM target is available
if ! rustup target list --installed | grep -q wasm32-unknown-unknown; then
    echo "❌ WASM target not found! Installing..."
    rustup target add wasm32-unknown-unknown
fi

echo "🎯 Building for WASM target..."
# Build the application
if ! cargo build --target wasm32-unknown-unknown --release; then
    echo "❌ Build failed!"
    exit 1
fi

# Small delay to ensure file is fully written
echo "⏳ Waiting for file system sync..."
sleep 1

# Create res directory if it doesn't exist
mkdir -p res

# Check if the WASM file was actually created in workspace root
WASM_PATH="../../target/wasm32-unknown-unknown/release/collection_storage_test.wasm"
if [ ! -f "$WASM_PATH" ]; then
    echo "❌ WASM file not found at $WASM_PATH"
    echo "📁 Current working directory: $(pwd)"
    echo "🔍 Looking for WASM files from workspace root:"
    find ../../target -name "*.wasm" 2>/dev/null || echo "No WASM files found in workspace root"
    exit 1
fi

# Copy the built WASM to res directory
cp "$WASM_PATH" res/

echo "✅ Build complete! WASM file: res/collection_storage_test.wasm"
echo "📊 File size: $(du -h res/collection_storage_test.wasm | cut -f1)"
