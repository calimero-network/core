#!/bin/bash
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

cargo build --target wasm32-unknown-unknown --profile app-release

cp $TARGET/wasm32-unknown-unknown/release/kv_store.wasm ./res/

if command -v wasm-opt > /dev/null; then
  wasm-opt -Oz ./res/kv_store.wasm -o ./res/kv_store.wasm
fi
