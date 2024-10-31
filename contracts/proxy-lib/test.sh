#!/bin/sh
set -e

./build-test-deps.sh

echo "Running tests..."
cargo test -- --nocapture
