#!/bin/sh

cd "$(dirname $0)"

# unit testing
cargo test

# sandbox testing
cd sandbox-rs
cargo run --example sandbox "../target/wasm32-unknown-unknown/release/package_manager.wasm"