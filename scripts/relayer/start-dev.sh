#!/bin/bash
set -e

# Check if dfx is installed
if ! command -v dfx &> /dev/null; then
    echo "dfx is required but not installed. Please install dfx: https://internetcomputer.org/docs/current/developer-docs/setup/install/"
    exit 1
fi

# Start dfx and wait for it to be ready
echo "Starting dfx..."
dfx start --clean --background --host 127.0.0.1:"${ICP_PORT:-4943}"

# Start Ethereum devnet and relayer
echo "Starting development services..."
docker-compose -f docker-compose.relayer.yml -f docker-compose.relayer.dev.yml up -d anvil

# Wait for dfx to be ready
echo "Waiting for dfx to be ready..."
timeout 60s bash -c 'until dfx ping; do sleep 2; done'

# Wait for anvil to be ready
echo "Waiting for anvil to be ready..."
timeout 60s bash -c 'until curl -s -f http://localhost:${ANVIL_PORT:-8545} > /dev/null; do sleep 2; done'

# Deploy contracts
echo "Deploying ICP contracts..."
./scripts/icp/deploy-devnet.sh

echo "Deploying Ethereum contracts..."
./scripts/ethereum/deploy-devnet.sh

# Start relayer
echo "Starting relayer..."
docker-compose -f docker-compose.relayer.yml -f docker-compose.relayer.dev.yml up -d relayer

echo "Development environment started successfully!"
echo "- Relayer API: http://localhost:${RELAYER_PORT:-63529}"
echo "- ICP dfx: http://localhost:${ICP_PORT:-4943}"
echo "- Ethereum Anvil: http://localhost:${ANVIL_PORT:-8545}"