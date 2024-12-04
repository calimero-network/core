#!/bin/sh
set -e

# Get the absolute path to the workspace root
WORKSPACE_ROOT="$(cd "$(dirname "$0")/../../../" && pwd)"

cd "$(dirname $0)"

echo "Building proxy contract..."
./build.sh

echo "Building test-ledger contract..."
(cd "$WORKSPACE_ROOT" && cargo build \
    --target wasm32-unknown-unknown \
    --profile app-release \
    -p mock_ledger)
mkdir -p mock/ledger/res
cp "$WORKSPACE_ROOT/target/wasm32-unknown-unknown/app-release/mock_ledger.wasm" mock/ledger/res/

echo "Building test-external contract..."
(cd "$WORKSPACE_ROOT" && cargo build \
    --target wasm32-unknown-unknown \
    --profile app-release \
    -p mock_external)
mkdir -p mock/external/res
cp "$WORKSPACE_ROOT/target/wasm32-unknown-unknown/app-release/mock_external.wasm" mock/external/res/

echo "Building context-config contract..."
(cd ../context-config && ./build.sh)
