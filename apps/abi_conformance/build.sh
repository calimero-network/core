#!/bin/bash
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-../../target}"

rustup target add wasm32-unknown-unknown

RUSTFLAGS="--remap-path-prefix $HOME=~" cargo build --target wasm32-unknown-unknown --profile app-release

mkdir -p res

cp $TARGET/wasm32-unknown-unknown/app-release/abi_conformance.wasm ./res/

if command -v wasm-opt > /dev/null; then
  wasm-opt -Oz ./res/abi_conformance.wasm -o ./res/abi_conformance.wasm
fi 
