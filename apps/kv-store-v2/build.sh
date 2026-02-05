#!/bin/bash
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

rustup target add wasm32-unknown-unknown

mkdir -p res

# Use app-profiling profile when WASM_PROFILING is set to preserve function names
if [ "${WASM_PROFILING:-false}" = "true" ]; then
    echo "Building with profiling profile "
    PROFILE="app-profiling"
else
    PROFILE="app-release"
fi

RUSTFLAGS="--remap-path-prefix $HOME=~" cargo build --target wasm32-unknown-unknown --profile "$PROFILE"

cp $TARGET/wasm32-unknown-unknown/$PROFILE/kv_store_v2.wasm ./res/

# Skip wasm-opt for profiling builds to preserve debug info
if [ "$PROFILE" = "app-release" ] && command -v wasm-opt > /dev/null; then
  wasm-opt -Oz ./res/kv_store_v2.wasm -o ./res/kv_store_v2.wasm
fi
