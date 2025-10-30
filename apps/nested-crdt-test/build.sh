#!/usr/bin/env bash
set -e

cd "$(dirname $0)"

cargo build \
    --target wasm32-unknown-unknown \
    --profile app-release

mkdir -p res

cp ../../target/wasm32-unknown-unknown/app-release/nested_crdt_test.wasm res/nested_crdt_test.wasm

echo "✅ nested-crdt-test built successfully"

