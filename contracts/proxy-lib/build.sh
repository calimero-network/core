#!/bin/sh
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

rustup target add wasm32-unknown-unknown

mkdir -p res

if [ "$1" = "--test" ]; then
  cargo build --target wasm32-unknown-unknown --profile app-release --features __internal_explode_size

  cp $TARGET/wasm32-unknown-unknown/app-release/proxy_lib.wasm ./res/proxy_lib_fat.wasm
fi

cargo build --target wasm32-unknown-unknown --profile app-release

cp $TARGET/wasm32-unknown-unknown/app-release/proxy_lib.wasm ./res/

if command -v wasm-opt > /dev/null; then
  wasm-opt -Oz ./res/proxy_lib.wasm -o ./res/proxy_lib.wasm
fi
