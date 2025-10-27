#!/bin/bash
set -e

cd "$(dirname $0)"

TARGET="${CARGO_TARGET_DIR:-target}"

rustup target add wasm32-unknown-unknown

cargo build --target wasm32-unknown-unknown --release

mkdir -p res

cp $TARGET/wasm32-unknown-unknown/release/xcall_example.wasm ./res/

