#!/bin/bash
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

rustup target add wasm32-unknown-unknown

mkdir -p res

RUSTFLAGS="--remap-path-prefix $HOME=~" cargo build --target wasm32-unknown-unknown --profile app-release

cp $TARGET/wasm32-unknown-unknown/app-release/kv_store_with_handlers.wasm ./res/

if command -v wasm-opt > /dev/null; then
  wasm-opt -Oz ./res/kv_store_with_handlers.wasm -o ./res/kv_store_with_handlers.wasm
fi
