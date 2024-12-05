#!/bin/sh
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../../target}"

rustup target add wasm32-unknown-unknown

cargo build --target wasm32-unknown-unknown --profile app-release

mkdir -p res

cp $TARGET/wasm32-unknown-unknown/app-release/calimero_context_config_icp.wasm ./res/

if command -v wasm-opt > /dev/null; then
  wasm-opt -Oz ./res/calimero_context_config_icp.wasm -o ./res/calimero_context_config_icp.wasm
fi

if command -v candid-extractor > /dev/null; then
  candid-extractor ./res/calimero_context_config_icp.wasm > ./res/calimero_context_config_icp.did
fi
