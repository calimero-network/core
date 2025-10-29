#!/usr/bin/env bash
set -e

cd "$(dirname $0)"

cargo build \
    --target wasm32-unknown-unknown \
    --profile app-release

mkdir -p res

cp ../../target/wasm32-unknown-unknown/app-release/team_metrics_custom.wasm res/team_metrics_custom.wasm

echo "✅ team-metrics-custom built successfully"

