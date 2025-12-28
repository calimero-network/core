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
# Get the full rustc commit hash from `rustc --version --verbose`
RUST_COMMIT_HASH=$(rustc -Vv | grep commit-hash | cut -d ' ' -f2)
RUST_MAP="rust"

# Deep source path identification
# This targets the actual 'library' folder to skip the `/lib/rustlib/src/rust/` nesting
LOCAL_RUST_SRC="$RUST_SYSROOT/lib/rustlib/src/rust"
VIRTUAL_RUST_SRC="/rustc/$RUST_COMMIT_HASH"

rustup target add wasm32-unknown-unknown

mkdir -p res

export RUSTFLAGS=" \
  --remap-path-prefix $WORKSPACE_ROOT=project \
  --remap-path-prefix $CARGO_HOME_DIR=cargo \
  --remap-path-prefix=$LOCAL_RUST_SRC=$RUST_MAP \
  --remap-path-prefix=$VIRTUAL_RUST_SRC=$RUST_MAP \
  -C debuginfo=0"

cargo build --target wasm32-unknown-unknown --profile app-release

cp $TARGET/wasm32-unknown-unknown/app-release/kv_store.wasm ./res/

if command -v wasm-opt > /dev/null; then
  wasm-opt -Oz --strip-debug --strip-producers ./res/kv_store.wasm -o ./res/kv_store.wasm
fi
