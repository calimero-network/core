#!/bin/bash
set -e

# Define variables
RPC_URL="http://localhost:8545"
PRIVATE_KEY="0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"

# Set contract artifact paths
# Assuming downloaded contracts are in the contracts directory
CONTRACTS_DIR=${CALIMERO_CONTRACTS_DIR:-contracts}
CONFIG_ARTIFACT="$CONTRACTS_DIR/ethereum/out/ContextConfig.sol/ContextConfig.json"
PROXY_ARTIFACT="$CONTRACTS_DIR/ethereum/out/ContextProxy.sol/ContextProxy.json"
MOCK_ARTIFACT="$CONTRACTS_DIR/ethereum/out/MockExternalContract.sol/MockExternalContract.json"

# Verify artifacts exist
if [ ! -f "$CONFIG_ARTIFACT" ] || [ ! -f "$PROXY_ARTIFACT" ] || [ ! -f "$MOCK_ARTIFACT" ]; then
    echo "Error: Ethereum contract artifacts not found."
    echo "Please run the download-contracts.sh script first or check paths:"
    echo "- $CONFIG_ARTIFACT"
    echo "- $PROXY_ARTIFACT"
    echo "- $MOCK_ARTIFACT"
    exit 1
fi

# Anvil's deterministic addresses (from default mnemonic)
DEPLOYER_ADDRESS="0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
# First contract deployed by this address will be at:
CONTEXT_CONFIG_ADDRESS="0x5FbDB2315678afecb367f032d93F642f64180aa3"
# Second contract will be at:
MOCK_CONTRACT_ADDRESS="0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512"

# Start Anvil in the background with &
echo "Starting Anvil..."
anvil --host 0.0.0.0 --port 8545 &

# Give Anvil a moment to start up
sleep 2

# Deploy ContextConfig
echo "Deploying ContextConfig..."
BYTECODE=$(jq -r '.bytecode.object' "$CONFIG_ARTIFACT")
ENCODED_ARGS=$(cast abi-encode "constructor(address)" $DEPLOYER_ADDRESS)
ENCODED_ARGS=${ENCODED_ARGS#0x}

DEPLOY_BYTECODE="${BYTECODE}${ENCODED_ARGS}"

# Deploy ContextConfig
cast send --private-key $PRIVATE_KEY --rpc-url $RPC_URL --create $DEPLOY_BYTECODE
# Get proxy bytecode
PROXY_BYTECODE=$(jq -r '.bytecode.object' "$PROXY_ARTIFACT")
# Set proxy code
cast send $CONTEXT_CONFIG_ADDRESS "setProxyCode(bytes)" $PROXY_BYTECODE --rpc-url $RPC_URL --private-key $PRIVATE_KEY

# Deploy MockExternalContract
echo "Deploying MockExternalContract..."
MOCK_BYTECODE=$(jq -r '.bytecode.object' "$MOCK_ARTIFACT")
cast send --private-key $PRIVATE_KEY --rpc-url $RPC_URL --create $MOCK_BYTECODE

echo "Anvil is running in the background on port 8545"
echo "To stop it, run: pkill -f anvil"
