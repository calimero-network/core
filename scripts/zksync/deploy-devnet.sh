#!/bin/bash
set -e

# Check if Docker is installed
if ! command -v docker &> /dev/null; then
    echo "Error: Docker is not installed. Please install Docker first:"
    echo "  - macOS: https://docs.docker.com/desktop/install/mac-install/"
    echo "  - Linux: https://docs.docker.com/engine/install/"
    echo "  - Windows: https://docs.docker.com/desktop/install/windows-install/"
    exit 1
fi

# Check if Docker daemon is running
if ! docker info &> /dev/null; then
    echo "Error: Docker daemon is not running. Please start Docker Desktop or the Docker service."
    exit 1
fi

# Function to cleanup on exit
cleanup() {
    echo "Cleaning up..."
    if [ -n "$NODE_PID" ] && ps -p "$NODE_PID" > /dev/null; then
        echo "Stopping zkSync node (PID: $NODE_PID)..."
        kill "$NODE_PID" 2>/dev/null || true
    fi
    # Additional cleanup if needed
    docker ps -q --filter "name=zksync" | xargs -r docker stop
    docker ps -a -q --filter "name=zksync" | xargs -r docker rm
}

# Set up trap to ensure cleanup runs on script exit
trap cleanup EXIT

echo "Installing zkSync CLI..."
npm install -g zksync-cli
zksync-cli --version

echo "Starting zkSync local node..."
# Start the node in the background and capture its output
(echo "1" && echo "") | zksync-cli dev start > zksync_output.log 2>&1 &
NODE_PID=$!

echo "Waiting for node to be ready..."
MAX_RETRIES=30
RETRY_COUNT=0
while ! curl -s http://localhost:8011 > /dev/null; do
    if [ $RETRY_COUNT -ge $MAX_RETRIES ]; then
        echo "Error: Timeout waiting for zkSync node to be ready"
        cat zksync_output.log
        exit 1
    fi
    echo "Waiting for node to be ready... ($(($MAX_RETRIES - $RETRY_COUNT)) seconds remaining)"
    sleep 1
    RETRY_COUNT=$((RETRY_COUNT + 1))
done

echo "Node is ready. Deploying contracts..."
cd contracts/zksync
forge build

# Create deployments directory
mkdir -p deployments/localhost

# Deploy contract and capture output
DEPLOY_OUTPUT=$(forge script script/Deploy.s.sol:DeployScript \
    --rpc-url http://localhost:8011 \
    --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
    --broadcast 2>&1 || true)

# Extract contract address from the output using grep and sed
CONTRACT_ID=$(echo "$DEPLOY_OUTPUT" | grep -o "0x[0-9a-fA-F]\{40\}")

if [ -z "$CONTRACT_ID" ]; then
    echo "Error: Failed to extract contract address from deployment output"
    echo "Deployment output:"
    echo "$DEPLOY_OUTPUT"
    exit 1
fi

# Save contract address to file
echo "{\"address\":\"$CONTRACT_ID\"}" > deployments/localhost/ContextConfig.json

echo "Contract ID: $CONTRACT_ID" 