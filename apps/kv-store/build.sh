#!/bin/bash
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

# Find the real workspace root
WORKSPACE_ROOT=$(cargo metadata --format-version 1 | jq -r '.workspace_root')
# Get the cargo home (usually ~/.cargo)
CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"
# Get the path to the Rust standard library source
RUST_SYSROOT=$(rustc --print sysroot)

rustup target add wasm32-unknown-unknown

mkdir -p res

RUSTFLAGS=" \
  --remap-path-prefix $PROJECT_ROOT=project \
  --remap-path-prefix $CARGO_HOME_DIR=cargo \
  --remap-path-prefix $RUST_SYSROOT=/rustc \
  -C debuginfo=0" \
cargo build --target wasm32-unknown-unknown --profile app-release

cp $TARGET/wasm32-unknown-unknown/app-release/kv_store.wasm ./res/

if command -v wasm-opt > /dev/null; then
  wasm-opt -Oz --strip-debug ./res/kv_store.wasm -o ./res/kv_store.wasm
fi
