#!/bin/bash
set -e

cd "$(dirname "$0")"

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

cp "$TARGET/wasm32-unknown-unknown/$PROFILE/scenario_identity_downgrade_v1.wasm" ./res/

if [ "$PROFILE" = "app-release" ] && command -v wasm-opt > /dev/null; then
  wasm-opt -Oz --enable-bulk-memory ./res/scenario_identity_downgrade_v1.wasm -o ./res/scenario_identity_downgrade_v1.wasm
fi

# Embed the emitted state schema into the wasm as the calimero_abi_v1 custom
# section. MUST run after wasm-opt (which strips unknown custom sections), so the
# node can read the schema at upgrade time for the L1 identity-downgrade gate.
cargo run -q -p mero-abi -- embed \
  ./res/scenario_identity_downgrade_v1.wasm \
  ./res/state-schema.json
