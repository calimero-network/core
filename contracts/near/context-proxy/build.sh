#!/bin/sh
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../../target}"

rustup target add wasm32-unknown-unknown

mkdir -p res

if [ "$1" = "--test" ]; then
  cargo build --target wasm32-unknown-unknown --profile app-release --features __internal_explode_size

  cp $TARGET/wasm32-unknown-unknown/app-release/calimero_context_proxy_near.wasm ./res/calimero_context_proxy_near_fat.wasm
fi

cargo build --target wasm32-unknown-unknown --profile app-release

cp $TARGET/wasm32-unknown-unknown/app-release/calimero_context_proxy_near.wasm ./res/

if command -v wasm-opt > /dev/null; then
  wasm-opt -Oz ./res/calimero_context_proxy_near.wasm -o ./res/calimero_context_proxy_near.wasm
fi
