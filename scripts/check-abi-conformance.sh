#!/bin/bash

set -e

echo "Building ABI conformance app..."
cargo build -p abi_conformance --target wasm32-unknown-unknown

echo "Building calimero-abi tool..."
cargo build -p calimero-abi

echo "Extracting ABI..."
./target/debug/calimero-abi extract target/wasm32-unknown-unknown/debug/abi_conformance.wasm -o /tmp/abi_conformance_extracted.json

echo "Comparing with expected ABI..."
if diff -u apps/abi_conformance/abi.expected.json /tmp/abi_conformance_extracted.json; then
    echo "✅ ABI conformance check passed!"
    exit 0
else
    echo "❌ ABI conformance check failed!"
    echo "Expected ABI: apps/abi_conformance/abi.expected.json"
    echo "Extracted ABI: /tmp/abi_conformance_extracted.json"
    exit 1
fi 