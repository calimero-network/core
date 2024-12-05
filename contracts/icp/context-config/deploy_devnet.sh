#!/bin/bash

# Build the contract
./build.sh

# Stop the replica
dfx stop

# Start the replica
dfx start --background

# Deploy the contract
dfx deploy
