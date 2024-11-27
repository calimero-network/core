#!/usr/bin/env bash

function generate_did() {
  local canister=$1

  cargo build --manifest-path="Cargo.toml" \
      --target wasm32-unknown-unknown \
      --release --package "$canister"

  candid-extractor "target/wasm32-unknown-unknown/release/$canister.wasm" > "$canister.did"
}

# The list of canisters of your project
CANISTERS=context_contract

for canister in $(echo $CANISTERS | sed "s/,/ /g")
do
    generate_did "$canister"
done