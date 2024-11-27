#!/bin/bash

# Exit on error
set -e

# Ensure we have the wasm32 target
rustup target add wasm32-unknown-unknown

# Build the contract
cargo build --target wasm32-unknown-unknown --release

# Generate the candid interface
candid-extractor .dfx/local/canisters/context_contract/context_contract.wasm > context_contract.did

# Stop the replica
dfx stop

# Start the replica
dfx start --background

# Deploy the contract
dfx deploy