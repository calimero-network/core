#!/bin/bash
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../../target}"

rustup target add wasm32-unknown-unknown

mkdir -p res

if [ "${WASM_PROFILING:-false}" = "true" ]; then
    echo "Building with profiling profile "
    PROFILE="app-profiling"
else
    PROFILE="app-release"
fi

RUSTFLAGS="--remap-path-prefix $HOME=~" cargo build --target wasm32-unknown-unknown --profile "$PROFILE"

cp $TARGET/wasm32-unknown-unknown/$PROFILE/migration_suite_v4_rename_field.wasm ./res/

if [ "$PROFILE" = "app-release" ] && command -v wasm-opt > /dev/null; then
  wasm-opt -Oz ./res/migration_suite_v4_rename_field.wasm -o ./res/migration_suite_v4_rename_field.wasm
fi
