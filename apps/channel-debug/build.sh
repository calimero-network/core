#!/bin/bash
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

rustup target add wasm32-unknown-unknown

cargo build --target wasm32-unknown-unknown --profile app-release

mkdir -p res

cp $TARGET/wasm32-unknown-unknown/app-release/channel_debug.wasm ./res/

if command -v wasm-opt > /dev/null; then
  # Try to optimize, but don't fail if it doesn't work
  wasm-opt -Oz ./res/channel_debug.wasm -o ./res/channel_debug.wasm || echo "Warning: wasm-opt optimization failed, using unoptimized WASM"
fi
