#!/bin/bash
set -e

# ZKsync Era devnet deployment script with contract deployment
# Required dependencies: foundry-zksync, jq
# Usage: ./deploy-devnet.sh

# Define variables
RPC_URL="http://localhost:8011"
PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80" # First rich account's private key from config

# Set contract artifact paths
CONTRACTS_DIR=${CALIMERO_CONTRACTS_DIR:-contracts}
CONFIG_ARTIFACT="$CONTRACTS_DIR/zksync/out/ContextConfig.sol/ContextConfig.json"
PROXY_ARTIFACT="$CONTRACTS_DIR/zksync/out/ContextProxy.sol/ContextProxy.json"
MOCK_ARTIFACT="$CONTRACTS_DIR/zksync/out/MockExternalContract.sol/MockExternalContract.json"

# Verify artifacts exist
if [ ! -f "$CONFIG_ARTIFACT" ] || [ ! -f "$PROXY_ARTIFACT" ] || [ ! -f "$MOCK_ARTIFACT" ]; then
    echo "Error: ZKsync contract artifacts not found."
    echo "Please run the download-contracts.sh script first or check paths:"
    echo "- $CONFIG_ARTIFACT"
    echo "- $PROXY_ARTIFACT"
    echo "- $MOCK_ARTIFACT"
    exit 1
fi

anvil-zksync --emulate-evm --protocol-version 27 fork --fork-url sepolia-testnet &

sleep 10

# Get deployer address for constructor argument
DEPLOYER_ADDRESS="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
# Expected deployed contract addresses
CONTEXT_CONFIG_ADDRESS="0x5FbDB2315678afecb367f032d93F642f64180aa3"
MOCK_CONTRACT_ADDRESS="0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"

# Deploy ContextConfig
echo "Deploying ContextConfig..."
BYTECODE=$(jq -r '.bytecode.object' "$CONFIG_ARTIFACT")
ENCODED_ARGS=$(cast abi-encode "constructor(address)" $DEPLOYER_ADDRESS)
ENCODED_ARGS=${ENCODED_ARGS#0x}

DEPLOY_BYTECODE="${BYTECODE}${ENCODED_ARGS}"

# Deploy ContextConfig
RESULT=$(cast send --private-key $PRIVATE_KEY --rpc-url $RPC_URL --create $DEPLOY_BYTECODE)
CONTEXT_CONFIG_DEPLOYED_ADDRESS=$(echo "$RESULT" | grep "contractAddress" | awk '{print $2}')
echo "ContextConfig deployed at: $CONTEXT_CONFIG_DEPLOYED_ADDRESS"

# Get proxy bytecode
PROXY_BYTECODE=$(jq -r '.bytecode.object' "$PROXY_ARTIFACT")
# Set proxy code
cast send $CONTEXT_CONFIG_DEPLOYED_ADDRESS "setProxyCode(bytes)" $PROXY_BYTECODE --rpc-url $RPC_URL --private-key $PRIVATE_KEY

# Deploy MockExternalContract
echo "Deploying MockExternalContract..."
MOCK_BYTECODE=$(jq -r '.bytecode.object' "$MOCK_ARTIFACT")
RESULT=$(cast send --private-key $PRIVATE_KEY --rpc-url $RPC_URL --create $MOCK_BYTECODE)
MOCK_CONTRACT_DEPLOYED_ADDRESS=$(echo "$RESULT" | grep "contractAddress" | awk '{print $2}')
echo "MockExternalContract deployed at: $MOCK_CONTRACT_DEPLOYED_ADDRESS"

echo "Deployment complete. Please update the following addresses in e2e-tests/config/config.json:"
echo "ContextConfig: $CONTEXT_CONFIG_DEPLOYED_ADDRESS"
echo "MockExternalContract: $MOCK_CONTRACT_DEPLOYED_ADDRESS"