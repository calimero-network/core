#!/bin/sh
set -e

PROXY_CONTRACT_PATH="./"
CONTEXT_CONFIG_CONTRACT_PATH="../context-config"

echo "Building proxy contract..."
(cd $PROXY_CONTRACT_PATH && ./build.sh)

echo "Building context-config contract..."
(cd $CONTEXT_CONFIG_CONTRACT_PATH && ./build.sh)

echo "Running tests..."
cargo test -- --nocapture
