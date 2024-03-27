#!/bin/sh
rustup target add wasm32-unknown-unknown
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

cargo build --target wasm32-unknown-unknown --profile app-release

mkdir -p res

cp $TARGET/wasm32-unknown-unknown/release/package_manager.wasm ./res/

if command -v wasm-opt > /dev/null; then
  wasm-opt -Oz ./res/package_manager.wasm -o ./res/packager_manager.wasm
fi
