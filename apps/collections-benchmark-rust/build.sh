#!/usr/bin/env bash
set -e

cd "$(dirname $0)"

cargo build \
    --target wasm32-unknown-unknown \
    --profile app-release

mkdir -p res

cp ../../target/wasm32-unknown-unknown/app-release/collections_benchmark_rust.wasm res/collections_benchmark_rust.wasm

echo "Built successfully: collections-benchmark-rust"
