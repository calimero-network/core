#!/bin/bash

# Build the contract
bash ./build.sh

# Generate the candid interface
candid-extractor res/context_contract.wasm > context_contract.did

# Stop the replica
dfx stop

# Start the replica
dfx start --background

# Deploy the contract
dfx deploy