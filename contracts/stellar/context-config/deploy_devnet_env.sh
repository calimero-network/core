#!/bin/bash
set -e

# Get the directory where the script is located
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"

# Check if stellar CLI is already installed
if ! command -v stellar &> /dev/null; then
    echo "Installing stellar CLI..."
    
    # Detect OS and architecture
    OS=$(uname -s | tr '[:upper:]' '[:lower:]')
    ARCH=$(uname -m)
    
    # Map architecture names
    case $ARCH in
        x86_64)
            ARCH="amd64"
            ;;
        aarch64|arm64)
            ARCH="arm64"
            ;;
    esac
    
    # Set binary name based on OS
    BINARY_NAME="stellar-cli-${OS}-${ARCH}"
    
    wget "https://github.com/stellar/stellar-cli/releases/download/v22.2.0/${BINARY_NAME}"
    chmod +x "${BINARY_NAME}"
    sudo mv "${BINARY_NAME}" /usr/local/bin/stellar
else
    echo "stellar is already installed"
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
echo "Account address: $ACCOUNT_ADDRESS"
SECRET_KEY=$(stellar keys show local)
echo "Secret key: $SECRET_KEY"

# Deploy the contract and capture the contract ID
CONTRACT_ID=$(stellar contract deploy \
    --wasm "$SCRIPT_DIR/res/calimero_context_config_stellar.wasm" \
    --source local \
    --network local \
    -- \
    --owner "$ACCOUNT_ADDRESS" \
    --ledger_id CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC | tail -n 1)
echo "Contract ID: $CONTRACT_ID"

# Invoke the contract to set proxy code
stellar contract invoke \
    --id "$CONTRACT_ID" \
    --source local \
    --network local \
    -- \
    set_proxy_code \
    --proxy-wasm-file-path "$SCRIPT_DIR/../context-proxy/res/calimero_context_proxy_stellar.wasm" \
    --owner "$ACCOUNT_ADDRESS"
