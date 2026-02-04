#!/bin/bash
set -e

cd "$(dirname $0)"
PROFILE="${PROFILE:-app-release}"
RUSTFLAGS="--remap-path-prefix $HOME=~" cargo build --target wasm32-unknown-unknown --profile "$PROFILE"

mkdir -p res
cp ../../target/wasm32-unknown-unknown/"$PROFILE"/state_visualization_test.wasm res/

# Try to optimize with wasm-opt, but don't fail if it doesn't work
if command -v wasm-opt &> /dev/null; then
    wasm-opt -Oz res/state_visualization_test.wasm -o res/state_visualization_test.wasm 2>/dev/null || {
        echo "Warning: wasm-opt optimization skipped (bulk memory operations not supported)"
    }
fi
