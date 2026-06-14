#!/bin/bash
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

rustup target add wasm32-unknown-unknown

mkdir -p res

cargo build --target wasm32-unknown-unknown --profile app-release

cp $TARGET/wasm32-unknown-unknown/app-release/xcall_example.wasm ./res/

if command -v wasm-opt > /dev/null; then
  wasm-opt -Oz --enable-bulk-memory ./res/xcall_example.wasm -o ./res/xcall_example.wasm
fi

# Embed the ABI into the wasm as the calimero_abi_v1 custom section so the node
# can read each method's `xcall_callable` flag and enforce the xcall entry-point
# gate. We embed the full abi.json (not state-schema.json, which strips methods).
# MUST run after wasm-opt, which drops unknown custom sections.
cargo run -q -p mero-abi -- embed \
  ./res/xcall_example.wasm \
  ./res/abi.json

