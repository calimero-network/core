#!/bin/bash
set -e

# Function to generate a new identity and return its principal
generate_identity() {
    local name=$1
    dfx identity new "$name" --storage-mode=plaintext || true
    dfx identity use "$name"
    dfx identity get-principal
}

# Function to get account ID from principal
get_account_id() {
    local principal=$1
    dfx ledger account-id --of-principal "$principal"
}

# Generate minting account
dfx identity new minting --storage-mode=plaintext || true
dfx identity use minting
MINTING_PRINCIPAL=$(dfx identity get-principal)
MINTING_ACCOUNT=$(get_account_id "$MINTING_PRINCIPAL")

# Generate initial account
dfx identity new initial --storage-mode=plaintext || true
dfx identity use initial
INITIAL_PRINCIPAL=$(dfx identity get-principal)
INITIAL_ACCOUNT=$(get_account_id "$INITIAL_PRINCIPAL")

# Generate archive controller account
dfx identity new archive --storage-mode=plaintext || true
dfx identity use archive
ARCHIVE_PRINCIPAL=$(dfx identity get-principal)

# Switch back to default identity
dfx identity use default

# Stop dfx and clean up all state
dfx stop
rm -rf .dfx
rm -rf ~/.config/dfx/replica-configuration/
rm -rf ~/.cache/dfinity/
# Remove canister_ids.json if it exists
if [ -f "canister_ids.json" ]; then
    rm canister_ids.json
fi

# Start dfx with clean state
dfx start --clean --background

# Define canister IDs
CONTEXT_ID="br5f7-7uaaa-aaaaa-qaaca-cai"
LEDGER_ID="be2us-64aaa-aaaaa-qaabq-cai"

# Create canisters
echo "Creating canisters..."
dfx canister create context_contract --specified-id "$CONTEXT_ID"
dfx canister create ledger --specified-id "$LEDGER_ID"

# Build contracts
echo "Building contracts..."
cd "$(dirname $0)"
./build.sh
cd ../context-proxy
./build.sh
cd ../context-config

# Prepare ledger initialization argument
LEDGER_INIT_ARG="(variant { Init = record { 
    minting_account = \"${MINTING_ACCOUNT}\"; 
    initial_values = vec { 
        record { \"${INITIAL_ACCOUNT}\"; record { e8s = 100_000_000_000 } } 
    }; 
    send_whitelist = vec {}; 
    transfer_fee = opt record { e8s = 10_000 }; 
    token_symbol = opt \"LICP\"; 
    token_name = opt \"Local Internet Computer Protocol Token\"; 
    archive_options = opt record { 
        trigger_threshold = 2000; 
        num_blocks_to_archive = 1000; 
        controller_id = principal \"${ARCHIVE_PRINCIPAL}\" 
    }; 
} })"

# Build and install canisters
dfx build
dfx canister install context_contract --mode=install
dfx canister install ledger --mode=install --argument "$LEDGER_INIT_ARG"

# Get the directory where the script is located
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Build path relative to the script location
WASM_FILE="${SCRIPT_DIR}/../context-proxy/res/calimero_context_proxy_icp.wasm"

# Verify file exists
if [ ! -f "$WASM_FILE" ]; then
    echo "Error: WASM file not found at: $WASM_FILE"
    exit 1
fi

# Then modify the script to use a consistent reading method
WASM_CONTENTS=$(xxd -p "$WASM_FILE" | tr -d '\n' | sed 's/\(..\)/\\\1/g')

TEMP_CMD=$(mktemp)
echo "(
  blob \"${WASM_CONTENTS}\",
  principal \"${LEDGER_ID}\"
)" > "$TEMP_CMD"

# Execute the command using the temporary file
dfx canister call context_contract set_proxy_code --argument-file "$TEMP_CMD"

# Clean up
rm "$TEMP_CMD"

# Print all relevant information at the end
echo -e "\n=== Deployment Summary ==="
echo "Context Contract ID: ${CONTEXT_ID}"
echo "Ledger Contract ID: ${LEDGER_ID}"
echo -e "\nAccount Information:"
echo "Minting Account: ${MINTING_ACCOUNT}"
echo "Initial Account: ${INITIAL_ACCOUNT}"
echo "Archive Principal: ${ARCHIVE_PRINCIPAL}"
echo -e "\nDeployment completed successfully!"
