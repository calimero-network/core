#!/bin/bash
set -e

# Build both contracts
echo "Building contracts..."
cd "$(dirname $0)"
./build.sh
cd ../context-proxy
./build.sh
cd ../context-config

# Stop and start dfx
echo "Restarting dfx..."
dfx stop
dfx start --background --clean

# Force remove existing canisters if they exist
echo "Cleaning up old canisters..."
dfx canister delete context_contract || true
dfx canister delete ledger || true

# Create and deploy canisters
echo "Deploying contracts..."
dfx canister create --all --force
dfx deploy

# Get the proxy wasm
echo "Reading proxy WASM..."
PROXY_WASM=$(xxd -p ../context-proxy/res/calimero_context_proxy_icp.wasm | tr -d '\n')

# Set proxy code in context config
echo "Setting proxy code in context config..."
dfx canister call context_contract set_proxy_code "(
  vec {$(echo $PROXY_WASM | sed 's/\([0-9a-f]\{2\}\)/0x\1;/g')},
  principal \"$LEDGER_ID\"
)"

echo "Deployment complete!"
