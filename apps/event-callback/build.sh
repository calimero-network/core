#!/bin/bash
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

rustup target add wasm32-unknown-unknown

mkdir -p res

cargo build --target wasm32-unknown-unknown --profile app-release

cp $TARGET/wasm32-unknown-unknown/app-release/event_callback.wasm ./res/

if command -v wasm-opt > /dev/null; then
  wasm-opt -Oz ./res/event_callback.wasm -o ./res/event_callback.wasm
fi
