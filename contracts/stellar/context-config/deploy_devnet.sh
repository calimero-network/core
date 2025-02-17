#!/bin/bash
set -e

# Get the directory where the script is located
cd "$(dirname "$0")"

# Check if required dependencies are installed
if ! command -v stellar &> /dev/null; then
    echo "Error: stellar CLI is not installed"
    exit 1
fi

if ! command -v docker &> /dev/null; then
    echo "Error: docker is not installed"
    exit 1
fi

# Check if Docker daemon is running
if ! docker info >/dev/null 2>&1; then
    echo "Error: Docker daemon is not running"
    exit 1
fi

# Start Stellar Quickstart container
docker run --rm -d -p 8000:8000 \
    --name stellar \
    stellar/quickstart:testing \
    --local --enable rpc --limits unlimited

# Wait for the container to be ready
echo "Waiting for Stellar container to be ready..."
while true; do
    # Check logs for completion
    LOGS=$(docker logs stellar 2>&1)
    
    if echo "$LOGS" | grep -q "horizon: ingestion caught up" && \
       echo "$LOGS" | grep -q "stellar-rpc: up and ready" && \
       echo "$LOGS" | grep -q "friendbot: started"; then
        # Additional check - try to access friendbot
        if curl -s "http://localhost:8000/friendbot" > /dev/null; then
            echo "Stellar Quickstart is ready!"
            # Add an extra sleep to ensure everything is truly ready
            sleep 5
            break
        fi
    fi
    echo "Waiting for services to be fully operational..."
    sleep 2
done

# Remove existing network and keys if they exist
stellar network rm local || true
stellar keys rm local || true

# Add network
stellar network add local \
    --rpc-url http://localhost:8000/soroban/rpc \
    --network-passphrase "Standalone Network ; February 2017"

# Generate and fund keys
stellar keys generate local --network local --fund
ACCOUNT_ADDRESS=$(stellar keys address local)
SECRET_KEY=$(stellar keys show local)

# Deploy the contract and capture the contract ID
CONTRACT_ID=$(stellar contract deploy \
    --wasm "./res/calimero_context_config_stellar.wasm" \
    --source local \
    --network local \
    -- \
    --owner "$ACCOUNT_ADDRESS" \
    --ledger_id CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC | tail -n 1)

# Invoke the contract to set proxy code
stellar contract invoke \
    --id "$CONTRACT_ID" \
    --source local \
    --network local \
    -- \
    set_proxy_code \
    --proxy-wasm-file-path "../context-proxy/res/calimero_context_proxy_stellar.wasm" \
    --owner "$ACCOUNT_ADDRESS"

# Update the config.json file with the new values
jq --arg contractId "$CONTRACT_ID" \
   --arg publicKey "$ACCOUNT_ADDRESS" \
   --arg secretKey "$SECRET_KEY" \
  '.protocolSandboxes[2].config.contextConfigContractId = $contractId |
   .protocolSandboxes[2].config.publicKey = $publicKey |
   .protocolSandboxes[2].config.secretKey = $secretKey' \
  ../../e2e-tests/config/config.json > tmp.json && mv tmp.json ../../e2e-tests/config/config.json

echo "Contract ID: $CONTRACT_ID"
echo "Account address: $ACCOUNT_ADDRESS"
echo "Secret key: $SECRET_KEY"
