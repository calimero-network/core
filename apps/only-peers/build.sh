#!/bin/bash
set -e

rustup target add wasm32-unknown-unknown

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

cargo build --target wasm32-unknown-unknown --profile app-release

mkdir -p res

cp $TARGET/wasm32-unknown-unknown/app-release/only_peers.wasm ./res/

if command -v wasm-opt > /dev/null; then
  wasm-opt -Oz ./res/only_peers.wasm -o ./res/only_peers.wasm
fi
