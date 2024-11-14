#!/bin/sh
set -e

cd "$(dirname $0)"

PROXY_CONTRACT_PATH="./"
TEST_CONTRACT_PATH="../test-counter"
CONTEXT_CONFIG_CONTRACT_PATH="../context-config"

echo "Building proxy contract..."
$PROXY_CONTRACT_PATH/build.sh --test

echo "Building test-counter contract..."
$TEST_CONTRACT_PATH/build.sh

echo "Building context-config contract..."
$CONTEXT_CONFIG_CONTRACT_PATH/build.sh
