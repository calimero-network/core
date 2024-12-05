#!/bin/sh
set -e

cd "$(dirname $0)"

CONTEXT_PROXY_CONTRACT_PATH="./"
CONTEXT_CONFIG_CONTRACT_PATH="../context-config"
TEST_CONTRACT_PATH="../test-counter"

echo "Building context-proxy contract..."
$CONTEXT_PROXY_CONTRACT_PATH/build.sh --test

echo "Building context-config contract..."
$CONTEXT_CONFIG_CONTRACT_PATH/build.sh

echo "Building test-counter contract..."
$TEST_CONTRACT_PATH/build.sh
