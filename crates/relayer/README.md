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

# Enable and configure Near protocol
export ENABLE_NEAR=true
export NEAR_NETWORK="testnet"
export NEAR_RPC_URL="https://rpc.testnet.near.org"
export NEAR_CONTRACT_ID="calimero-context-config.testnet"
export NEAR_ACCOUNT_ID="your-account.testnet"
export NEAR_PUBLIC_KEY="ed25519:..."
export NEAR_SECRET_KEY="ed25519:..."

# Enable and configure Starknet protocol
export ENABLE_STARKNET=true
export STARKNET_NETWORK="sepolia"
export STARKNET_RPC_URL="https://free-rpc.nethermind.io/sepolia-juno/"
# ... other Starknet settings

# Similar for ICP and Ethereum
export ENABLE_ICP=false
export ENABLE_ETHEREUM=false
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

## Docker Usage

The relayer can be run in Docker:

```bash
# Build the image
docker build -f Dockerfile.relayer -t calimero-relayer .

# Run with environment variables
docker run -p 63529:63529 \
  -e ENABLE_NEAR=true \
  -e NEAR_ACCOUNT_ID=your-account.testnet \
  -e NEAR_PUBLIC_KEY=ed25519:... \
  -e NEAR_SECRET_KEY=ed25519:... \
  calimero-relayer

# Run with config file
docker run -p 63529:63529 \
  -v $(pwd)/relayer-config.toml:/data/config.toml \
  calimero-relayer --config /data/config.toml
```

## Environment Variables

- `RELAYER_LISTEN`: Listen address (default: "0.0.0.0:63529")
- `PORT`: Override port (used by `addr_from_str` parser)
- `RUST_LOG`: Set logging level (e.g., `RUST_LOG=info`)

For each protocol, use the pattern: `ENABLE_{PROTOCOL}`, `{PROTOCOL}_NETWORK`, `{PROTOCOL}_RPC_URL`, etc.

## API

The relayer exposes a single HTTP POST endpoint at `/` that accepts JSON requests in the format expected by the Calimero context configuration client.

## Architecture

The relayer is a self-contained binary that:

1. **Loads Configuration**: From environment variables or config file
2. **Initializes Blockchain Clients**: Creates transport clients for enabled protocols
3. **Runs HTTP Server**: Forwards incoming requests to appropriate blockchain transports
4. **Operates Independently**: No dependency on `merod` node configuration

The relayer builds its own `calimero-context-config::client::Client` from its standalone configuration and handles all blockchain interactions directly.