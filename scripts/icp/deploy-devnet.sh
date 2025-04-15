#!/bin/bash

# This script is used to initialise a local ICP devnet with Calimero contracts.
# Required dependencies: dfx
# Usage: ./deploy-devnet-icp.sh [contracts_dir]
# Arguments: contracts_dir - Directory containing the Calimero contracts (default: contracts/icp)

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

CONTRACTS_DIR=${1:-contracts/icp}
CONTEXT_CONFIG_CONTRACT="calimero_context_config_icp"
CONTEXT_PROXY_CONTRACT="calimero_context_proxy_icp"
MOCK_EXTERNAL_CONTRACT="calimero_mock_external_icp"

if ! command -v dfx &> /dev/null; then
    echo "dfx is required but not installed. Please install dfx: https://internetcomputer.org/docs/current/developer-docs/setup/install/" >&2
    exit 1
fi

cd "${CONTRACTS_DIR}"

dfxvm default 0.24.3

# Stop dfx and clean up all state
dfx stop
rm -rf .dfx
rm -rf ~/.config/dfx/replica-configuration/
rm -rf ~/.config/dfx/identity/minting
rm -rf ~/.config/dfx/identity/initial
rm -rf ~/.config/dfx/identity/archive
rm -rf ~/.cache/dfinity/
rm -rf ~/.config/dfx/
dfxvm default 0.24.3

if [ -f "canister_ids.json" ]; then
    rm canister_ids.json
fi

echo "Generating dfx.json file..."
cat <<EOL >dfx.json
{
  "canisters": {
    "context_contract": {
      "type": "custom",
      "candid": "$CONTEXT_CONFIG_CONTRACT.did",
      "wasm": "$CONTEXT_CONFIG_CONTRACT.wasm"
    },
    "ledger": {
      "type": "custom",
      "candid": "https://raw.githubusercontent.com/dfinity/ic/aba60ffbc46acfc8990bf4d5685c1360bd7026b9/rs/ledger_suite/icp/ledger.did",
      "wasm": "https://download.dfinity.systems/ic/aba60ffbc46acfc8990bf4d5685c1360bd7026b9/canisters/ledger-canister.wasm.gz"
    },
    "mock_external_contract": {
      "type": "custom",
      "wasm": "$MOCK_EXTERNAL_CONTRACT.wasm",
      "candid": "$MOCK_EXTERNAL_CONTRACT.did"
    }
  },
  "defaults": {
    "build": {
      "args": "",
      "packtool": ""
    }
  },
  "networks": {
    "local": {
      "bind": "127.0.0.1:4943",
      "type": "persistent"
    }
  },
  "routing_table": {
    "start_canister_id": "aaaaa-aa",
    "end_canister_id": "zzzzz-zz"
  },
  "metadata": [
      {
        "name": "candid:service"
      }
  ],
  "version": 1
}
EOL

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

# Generate test recipient account
dfx identity new recipient --storage-mode=plaintext || true
dfx identity use recipient
RECIPIENT_PRINCIPAL=$(dfx identity get-principal)

# Switch back to default identity
dfx identity use default

# Start dfx with clean state
dfx start --clean --background --host 127.0.0.1:"${ICP_PORT:-4943}"

# Create initial identity if needed
dfx identity new --storage-mode=plaintext minting || true
# dfx identity use minting

echo "Creating and deploying canister..."
dfx canister create context_contract
dfx canister create ledger
dfx canister create mock_external_contract
# Get the context ID
CONTEXT_ID=$(dfx canister id context_contract)
# Get the wallet ID and seed it
WALLET_ID=$(dfx identity get-wallet)
MOCK_EXTERNAL_ID=$(dfx canister id mock_external_contract)

# abricate cycles for the wallet
dfx ledger fabricate-cycles --canister $WALLET_ID --amount 2000000

# Transfer cycles from wallet to context contract
dfx canister deposit-cycles 1000000000000000000 $CONTEXT_ID

echo "Done! Cycles transferred to context contract: $CONTEXT_ID"

# Get the IDs
CONTEXT_ID=$(dfx canister id context_contract)
LEDGER_ID=$(dfx canister id ledger)

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

# Build canisters
dfx build

# First install the ledger canister
dfx canister install ledger --mode=install --argument "$LEDGER_INIT_ARG"

# Get the ledger ID and install context contract with it
LEDGER_ID=$(dfx canister id ledger)
dfx canister install context_contract --mode=install --argument "(principal \"${LEDGER_ID}\")"

# Verify file exists
if [ ! -f "$CONTEXT_PROXY_CONTRACT.wasm" ]; then
    echo "Error: WASM file not found at: $CONTEXT_PROXY_CONTRACT.wasm"
    exit 1
fi

# Then modify the script to use a consistent reading method
WASM_CONTENTS=$(xxd -p "$CONTEXT_PROXY_CONTRACT.wasm" | tr -d '\n' | sed 's/\(..\)/\\\1/g')

# Execute the command using the temporary file
dfx canister call context_contract set_proxy_code --argument-file <(
  echo "(
    blob \"${WASM_CONTENTS}\"
  )"
)

# Install mock external contract
dfx canister install mock_external_contract --mode=install --argument "(principal \"${LEDGER_ID}\")"

# Print all relevant information at the end
echo -e "\n=== Deployment Summary ==="
echo "Context Contract ID: ${CONTEXT_ID}"
echo "Ledger Contract ID: ${LEDGER_ID}"
echo -e "\nAccount Information:"
echo "Minting Account: ${MINTING_ACCOUNT}"
echo "Initial Account: ${INITIAL_ACCOUNT}"
echo "Archive Principal: ${ARCHIVE_PRINCIPAL}"
echo "Recipient Principal: ${RECIPIENT_PRINCIPAL}"
echo "Mock External Contract ID: ${MOCK_EXTERNAL_ID}"
echo -e "\nDeployment completed successfully!"
