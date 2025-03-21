#!/bin/bash

# This script is used to initialise a local ICP devnet with Calimero contracts.
# Required dependencies: dfx
# Usage: ./nuke-devnet.sh [contracts_dir]
# Arguments: contracts_dir - Directory containing the Calimero contracts (default: contracts/icp)

set -e

CONTRACTS_DIR=${1:-contracts/icp}

if ! command -v dfx &> /dev/null; then
    echo "dfx is required but not installed. Please install dfx: https://internetcomputer.org/docs/current/developer-docs/setup/install/" >&2
    exit 1
fi

cd "${CONTRACTS_DIR}"

dfxvm default 0.24.3

# Stop dfx and clean up all state
dfx stop
rm -rf .dfx
rm -rf ~/.config/dfx/replica-configuration/
rm -rf ~/.config/dfx/identity/minting
rm -rf ~/.config/dfx/identity/initial
rm -rf ~/.config/dfx/identity/archive
rm -rf ~/.cache/dfinity/
rm -rf ~/.config/dfx/
