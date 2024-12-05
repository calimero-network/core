#!/bin/sh
set -e

CONTRACT_PATH="./"

echo "Building context-config contract..."
(cd $CONTRACT_PATH && ./build.sh)

echo "Running tests..."
cargo test -- --nocapture
