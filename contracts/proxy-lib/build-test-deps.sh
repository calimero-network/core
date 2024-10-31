#!/bin/sh
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

PROXY_CONTRACT_PATH="./"
TEST_CONTRACT_PATH="../test-counter"
CONTEXT_CONFIG_CONTRACT_PATH="../context-config"

echo "Building proxy contract..."
(cd $PROXY_CONTRACT_PATH && ./build.sh)

echo "Building proxy contract..."
(cd $TEST_CONTRACT_PATH && ./build.sh)

echo "Building context-config contract..."
(cd $CONTEXT_CONFIG_CONTRACT_PATH && ./build.sh)