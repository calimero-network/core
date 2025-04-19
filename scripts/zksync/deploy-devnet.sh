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
echo "Encoding constructor arguments..."
ENCODED_ARGS=$(cast abi-encode "constructor(address)" $DEPLOYER_ADDRESS)
echo "Encoded args: $ENCODED_ARGS"
ENCODED_ARGS=${ENCODED_ARGS#0x}
echo "Encoded args (without 0x): $ENCODED_ARGS"

DEPLOY_BYTECODE="${BYTECODE}${ENCODED_ARGS}"
echo "Full deploy bytecode length: ${#DEPLOY_BYTECODE}"

# Deploy ContextConfig
RESULT=$(cast send --private-key $PRIVATE_KEY --rpc-url $RPC_URL --create $DEPLOY_BYTECODE)
CONTEXT_CONFIG_DEPLOYED_ADDRESS=$(echo "$RESULT" | grep "contractAddress" | awk '{print $2}')
echo "ContextConfig deployed at: $CONTEXT_CONFIG_DEPLOYED_ADDRESS"

# Verify ContextConfig deployment
echo "Verifying ContextConfig deployment..."
echo "Checking contract bytecode..."
DEPLOYED_BYTECODE=$(cast code --rpc-url $RPC_URL $CONTEXT_CONFIG_DEPLOYED_ADDRESS)
if [ -z "$DEPLOYED_BYTECODE" ] || [ "$DEPLOYED_BYTECODE" = "0x" ]; then
    echo "Error: No bytecode found at deployed address"
    exit 1
fi
echo "✅ Contract bytecode verified"

# Get owner() function selector (first 4 bytes of keccak256("owner()"))
OWNER_SELECTOR=$(cast keccak "owner()" | cut -c 1-10)
echo "Owner selector: $OWNER_SELECTOR"

# Call owner() function with proper selector
OWNER_RESULT=$(cast call --rpc-url $RPC_URL $CONTEXT_CONFIG_DEPLOYED_ADDRESS "$OWNER_SELECTOR")
echo "Owner result: $OWNER_RESULT"
echo "Expected owner: $DEPLOYER_ADDRESS"

# Extract the last 40 characters (20 bytes) from the result and convert to lowercase
OWNER_ADDRESS=$(echo "$OWNER_RESULT" | sed 's/0x000000000000000000000000\(.*\)/\1/' | tr '[:upper:]' '[:lower:]')
EXPECTED_ADDRESS=$(echo "${DEPLOYER_ADDRESS#0x}" | tr '[:upper:]' '[:lower:]')

if [ "$OWNER_ADDRESS" != "$EXPECTED_ADDRESS" ]; then
    echo "Error: ContextConfig deployment verification failed - owner mismatch"
    echo "Got: $OWNER_RESULT"
    echo "Expected owner: $DEPLOYER_ADDRESS"
    exit 1
fi
echo "✅ ContextConfig deployment verified"

# Get proxy bytecode
PROXY_BYTECODE=$(jq -r '.bytecode.object' "$PROXY_ARTIFACT")
# Set proxy code
cast send $CONTEXT_CONFIG_DEPLOYED_ADDRESS "setProxyCode(bytes)" $PROXY_BYTECODE --rpc-url $RPC_URL --private-key $PRIVATE_KEY

# Verify proxy code was set
echo "Verifying proxy code was set..."
# Get proxyCode() function selector (first 4 bytes of keccak256("proxyCode()"))
PROXY_CODE_SELECTOR=$(cast keccak "proxyCode()" | cut -c 1-10)
echo "Proxy code selector: $PROXY_CODE_SELECTOR"

# Call proxyCode() function with proper selector
PROXY_CODE_RESULT=$(cast call --rpc-url $RPC_URL $CONTEXT_CONFIG_DEPLOYED_ADDRESS "$PROXY_CODE_SELECTOR")
if [ -z "$PROXY_CODE_RESULT" ] || [ "$PROXY_CODE_RESULT" = "0x" ]; then
    echo "Error: Proxy code verification failed"
    echo "Got: $PROXY_CODE_RESULT"
    exit 1
fi
echo "✅ Proxy code verified"

# Deploy MockExternalContract
echo "Deploying MockExternalContract..."
MOCK_BYTECODE=$(jq -r '.bytecode.object' "$MOCK_ARTIFACT")
RESULT=$(cast send --private-key $PRIVATE_KEY --rpc-url $RPC_URL --create $MOCK_BYTECODE)
MOCK_CONTRACT_DEPLOYED_ADDRESS=$(echo "$RESULT" | grep "contractAddress" | awk '{print $2}')
echo "MockExternalContract deployed at: $MOCK_CONTRACT_DEPLOYED_ADDRESS"

# Verify MockExternalContract deployment
echo "Verifying MockExternalContract deployment..."
# Test setting and getting a value
TEST_KEY="testKey"
TEST_VALUE="testValue"

# Set a value
echo "Setting test value..."
cast send --rpc-url $RPC_URL --private-key $PRIVATE_KEY $MOCK_CONTRACT_DEPLOYED_ADDRESS "setValueNoDeposit(string,string)" "$TEST_KEY" "$TEST_VALUE"

# Get the value back
echo "Getting test value..."
RESULT=$(cast call --rpc-url $RPC_URL $MOCK_CONTRACT_DEPLOYED_ADDRESS "getValue(string)" "$TEST_KEY")
if ! echo "$RESULT" | grep -q "$TEST_VALUE"; then
    echo "Error: MockExternalContract deployment verification failed - value mismatch"
    exit 1
fi
echo "✅ MockExternalContract deployment verified"

echo "Deployment complete. Please update the following addresses in e2e-tests/config/config.json:"
echo "ContextConfig: $CONTEXT_CONFIG_DEPLOYED_ADDRESS"
echo "MockExternalContract: $MOCK_CONTRACT_DEPLOYED_ADDRESS"