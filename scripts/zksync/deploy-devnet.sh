#!/bin/bash
set -e

echo "Installing zkSync CLI..."
npm install -g zksync-cli
zksync-cli --version

echo "Starting zkSync local node..."
zksync-cli dev start

echo "Deploying contracts..."
cd contracts/zksync
forge build
forge script script/Deploy.s.sol:DeployScript --rpc-url http://localhost:8011 --broadcast

# Output contract address for GitHub Actions
CONTRACT_ID=$(cat deployments/localhost/ContextConfig.json | jq -r '.address')
echo "Contract ID: $CONTRACT_ID" 