#!/bin/bash
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

rustup target add wasm32-unknown-unknown

mkdir -p res

RUSTFLAGS="--remap-path-prefix $HOME=~" cargo build --target wasm32-unknown-unknown --profile app-release

cp $TARGET/wasm32-unknown-unknown/app-release/components_demo.wasm ./res/

if command -v wasm-opt > /dev/null; then
  wasm-opt -Oz --enable-bulk-memory ./res/components_demo.wasm -o ./res/components_demo.wasm
fi
