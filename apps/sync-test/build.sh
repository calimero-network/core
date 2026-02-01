#!/bin/bash

set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

cargo build --target wasm32-unknown-unknown --profile app-release

mkdir -p res

cp "$TARGET/wasm32-unknown-unknown/app-release/sync_test.wasm" res/sync_test.wasm

# Skip wasm-opt for now - it requires --enable-bulk-memory-opt
# if command -v wasm-opt >/dev/null 2>&1; then
#     wasm-opt -Oz --enable-bulk-memory res/sync_test.wasm -o res/sync_test.wasm
# fi

ls -la res/sync_test.wasm
