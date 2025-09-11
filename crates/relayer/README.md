# Calimero Relayer

The `calimero-relayer` is a standalone relay server for external client interactions with the Calimero network. It forwards requests to the appropriate blockchain protocols based on its own configuration and operates independently of the main `merod` node.

## Usage

### Standalone Binary

The relayer is a self-contained standalone service:

```bash
# Build the binary
cargo build --bin calimero-relayer

# Run with default settings (listens on 0.0.0.0:63529, Near testnet enabled with default credentials)
calimero-relayer

# Run with custom address
calimero-relayer --listen 127.0.0.1:8080

# Load configuration from file
calimero-relayer --config /path/to/relayer-config.toml

# Get help
calimero-relayer --help
```

### Configuration

The relayer can be configured in two ways:

#### 1. Default Configuration (Zero Setup)

The relayer works **out-of-the-box** with sensible defaults:

```bash
# Just run it! Near testnet enabled with working credentials
calimero-relayer
```

**Default Configuration:**
- **Near Protocol**: ✅ Enabled with testnet credentials
- **Starknet**: ❌ Disabled (enable with `ENABLE_STARKNET=true`)
- **ICP**: ❌ Disabled (enable with `ENABLE_ICP=true`)  
- **Ethereum**: ❌ Disabled (enable with `ENABLE_ETHEREUM=true`)

All protocols include working default testnet credentials when enabled.

#### 2. Environment Variables (Custom Configuration)

To customize protocols or credentials, use environment variables:

```bash
# Basic relayer settings
export RELAYER_LISTEN="0.0.0.0:63529"

# Near Protocol Configuration
export ENABLE_NEAR=true
export NEAR_NETWORK="testnet"
export NEAR_RPC_URL="https://rpc.testnet.near.org"
export NEAR_CONTRACT_ID="calimero-context-config.testnet"
export NEAR_ACCOUNT_ID="your-account.testnet"
export NEAR_PUBLIC_KEY="ed25519:98GtfF5gBPUvBWNgz8N8WNEjXRgBhFLuSQ5MnFDEjJ8x"
export NEAR_SECRET_KEY="ed25519:4YdVWc7hgBUWwE9kXd4SPKmCztbGkMdHfZL2fDWw8L7g..."

# Starknet Protocol Configuration
export ENABLE_STARKNET=true
export STARKNET_NETWORK="sepolia"
export STARKNET_RPC_URL="https://free-rpc.nethermind.io/sepolia-juno/"
export STARKNET_CONTRACT_ID="0x1b991ee006e2d1e372ab96d0a957401fa200358f317b681df2948f30e17c29c"
export STARKNET_ACCOUNT_ID="0x01cf4d57ba01109f018dec3ea079a38fc08b0f8a78eed0d4c5e5fb22928dbc8c"
export STARKNET_PUBLIC_KEY="0x02c5dbad71c92a45cc4b40573ae661f8147869a91d57b8d9b8f48c8af7f83159"
export STARKNET_SECRET_KEY="0x0178eb2a625c0a8d85b0a5fd69fc879f9884f5205ad9d1ba41db0d7d1a77950a"

# ICP Protocol Configuration
export ENABLE_ICP=true
export ICP_NETWORK="local"
export ICP_RPC_URL="http://127.0.0.1:4943"
export ICP_CONTRACT_ID="bkyz2-fmaaa-aaaaa-qaaaq-cai"
export ICP_ACCOUNT_ID="rdmx6-jaaaa-aaaaa-aaadq-cai"
export ICP_PUBLIC_KEY="MCowBQYDK2VwAyEAL8XDEY1gGOWvv/0h01tW/ZV14qYY7GrHJF3pZoNxmHE="
export ICP_SECRET_KEY="MFECAQEwBQYDK2VwBCIEIJKDIfd1Ybt7xliQlRmXZGRWG8dJ1Dl9qKGT0pOhMwPjaE30"

# Ethereum Protocol Configuration
export ENABLE_ETHEREUM=true
export ETHEREUM_NETWORK="sepolia"
export ETHEREUM_RPC_URL="https://sepolia.drpc.org"
export ETHEREUM_CONTRACT_ID="0x83365DE41E1247511F4C5D10Fb1AFe59b96aD4dB"
export ETHEREUM_ACCOUNT_ID="0x8ba1f109551bD432803012645Hac136c22C177ec"
export ETHEREUM_SECRET_KEY="0ac1e735c1ca39db4a9c54d4edf2c6a50a75a3b3dce1cd2a64e8f5a44d1e2d2c"
```

#### 3. Configuration File

Create a TOML or JSON configuration file:

```toml
# relayer-config.toml
listen = "0.0.0.0:63529"

[protocols.near]
network = "testnet"
rpc_url = "https://rpc.testnet.near.org"
contract_id = "calimero-context-config.testnet"

[protocols.near.credentials]
type = "near"
account_id = "your-account.testnet"
public_key = "ed25519:..."
secret_key = "ed25519:..."

[protocols.starknet]
network = "sepolia"
rpc_url = "https://free-rpc.nethermind.io/sepolia-juno/"
contract_id = "0x1b991ee006e2d1e372ab96d0a957401fa200358f317b681df2948f30e17c29c"
# ... credentials
```

Then run:
```bash
calimero-relayer --config relayer-config.toml
```

### Migration from Merod

The relayer functionality has been **completely removed from merod** and is now only available as a standalone service. The relayer no longer depends on `merod` configuration.

```bash
# OLD (no longer available):
# merod --node-name my-node relay --listen 127.0.0.1:63529

# NEW (standalone with own config):
calimero-relayer --listen 127.0.0.1:63529
```


## Environment Variables

### General Settings
- `RELAYER_LISTEN`: Listen address (default: "0.0.0.0:63529")
- `PORT`: Override port (used by `addr_from_str` parser)
- `RUST_LOG`: Set logging level (e.g., `RUST_LOG=info`)

### Protocol Configuration Pattern
For each protocol (`NEAR`, `STARKNET`, `ICP`, `ETHEREUM`), use these patterns:

**Basic Protocol Settings:**
- `ENABLE_{PROTOCOL}`: Enable/disable protocol (true/false)
- `{PROTOCOL}_NETWORK`: Network name (e.g., "testnet", "mainnet", "local")
- `{PROTOCOL}_RPC_URL`: RPC endpoint URL
- `{PROTOCOL}_CONTRACT_ID`: Contract address/ID

**Protocol Credentials:**
- `{PROTOCOL}_ACCOUNT_ID`: Account address/ID/principal
- `{PROTOCOL}_PUBLIC_KEY`: Public key (Near, Starknet, ICP only)
- `{PROTOCOL}_SECRET_KEY`: Private/secret key

**Examples:**
```bash
# Near
ENABLE_NEAR=true
NEAR_ACCOUNT_ID=dev-1642425627065-33437663923179
NEAR_PUBLIC_KEY=ed25519:98GtfF5gBPUvBWNgz8N8WNEjXRgBhFLuSQ5MnFDEjJ8x
NEAR_SECRET_KEY=ed25519:4YdVWc7hgBUWwE9kXd4SPKmCztbGkMdHfZL2fDWw8L7g...

# Ethereum  
ENABLE_ETHEREUM=true
ETHEREUM_ACCOUNT_ID=0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
ETHEREUM_SECRET_KEY=ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80
```

## API

The relayer exposes a single HTTP POST endpoint at `/` that accepts JSON requests in the format expected by the Calimero context configuration client.

## Architecture

The relayer is a self-contained binary that:

1. **Loads Configuration**: From environment variables or config file
2. **Initializes Blockchain Clients**: Creates transport clients for enabled protocols
3. **Runs HTTP Server**: Forwards incoming requests to appropriate blockchain transports
4. **Operates Independently**: No dependency on `merod` node configuration

The relayer builds its own `calimero-context-config::client::Client` from its standalone configuration and handles all blockchain interactions directly.