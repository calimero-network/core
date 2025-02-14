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

echo "Checking dependencies..."
# Check for required commands
REQUIRED_COMMANDS="dfx cargo candid-extractor"

for cmd in $REQUIRED_COMMANDS; do
    if ! command -v $cmd >/dev/null 2>&1; then
        case $cmd in
            "dfx")
                echo "dfx is required but not installed. Please install dfx: https://internetcomputer.org/docs/current/developer-docs/setup/install/" >&2
                ;;
            "cargo")
                echo "cargo is required but not installed. Please install Rust: https://rustup.rs/" >&2
                ;;
            "candid-extractor")
                echo "candid-extractor is required but not installed. Please install: cargo install candid-extractor" >&2
                ;;
        esac
        exit 1
    fi
done


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
# Remove canister_ids.json if it exists
if [ -f "canister_ids.json" ]; then
    rm canister_ids.json
fi

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

# Get the context ID
CONTEXT_ID=$(dfx canister id context_contract)
# Get the wallet ID and seed it
WALLET_ID=$(dfx identity get-wallet)

# abricate cycles for the wallet
dfx ledger fabricate-cycles --canister $WALLET_ID --amount 200000

# Transfer cycles from wallet to context contract
dfx canister deposit-cycles 1000000000000000000 $CONTEXT_ID

echo "Done! Cycles transferred to context contract: $CONTEXT_ID"

# Get the IDs
CONTEXT_ID=$(dfx canister id context_contract)
LEDGER_ID=$(dfx canister id ledger)

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

# First install the ledger canister
dfx canister install ledger --mode=install --argument "$LEDGER_INIT_ARG"

# Get the ledger ID and install context contract with it
LEDGER_ID=$(dfx canister id ledger)
dfx canister install context_contract --mode=install --argument "(principal \"${LEDGER_ID}\")"

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

# Execute the command using the temporary file
dfx canister call context_contract set_proxy_code --argument-file <(
  echo "(
    blob \"${WASM_CONTENTS}\"
  )"
)

# Print all relevant information at the end
echo -e "\n=== Deployment Summary ==="
echo "Context Contract ID: ${CONTEXT_ID}"
echo "Ledger Contract ID: ${LEDGER_ID}"
echo -e "\nAccount Information:"
echo "Minting Account: ${MINTING_ACCOUNT}"
echo "Initial Account: ${INITIAL_ACCOUNT}"
echo "Archive Principal: ${ARCHIVE_PRINCIPAL}"
echo "Recipient Principal: ${RECIPIENT_PRINCIPAL}"
echo -e "\nDeployment completed successfully!"
