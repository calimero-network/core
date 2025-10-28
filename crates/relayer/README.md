# Calimero Relayer

The `mero-relayer` is a standalone relay server for external client interactions with the Calimero network. It forwards requests to the appropriate blockchain protocols based on its own configuration and operates independently of the main `merod` node.

## Usage

### Standalone Binary

The relayer is a self-contained standalone service:

```bash
# Build the binary
cargo build --bin mero-relayer

# Run with default settings (listens on 0.0.0.0:63529, Near testnet enabled with default credentials)
mero-relayer

# Run with custom address
mero-relayer --listen 127.0.0.1:8080

# Load configuration from file
mero-relayer --config /path/to/relayer-config.toml

# Get help
mero-relayer --help
```

### Configuration

The relayer can be configured in two ways:

#### 1. Default Configuration (Zero Setup)

The relayer works **out-of-the-box** with sensible defaults:

```bash
# Just run it! Near testnet enabled with working credentials
mero-relayer
```

**Default Configuration:**
- **Near Protocol**: ✅ Enabled (requires `NEAR_DEFAULT_SECRET_KEY` environment variable)
- **Starknet**: ❌ Disabled (enable with `ENABLE_STARKNET=true`)
- **ICP**: ❌ Disabled (enable with `ENABLE_ICP=true`)
- **Ethereum**: ❌ Disabled (enable with `ENABLE_ETHEREUM=true`)

**Security Note**: For security reasons, secret keys are **never** hardcoded. Even default configurations require environment variables for credentials.

**Environment Variables Setup:**

Create your environment file from the template:
```bash
# Create .env file from template
cp .env.example .env

# Edit .env file and fill in your actual credentials
nano .env  # or your preferred editor
```

#### 2. Environment Variables (Custom Configuration)

To customize protocols or credentials, use environment variables:

```bash
# Basic relayer settings
export RELAYER_LISTEN_URL="0.0.0.0:63529"

# Near Protocol Configuration
export ENABLE_NEAR=true
export NEAR_NETWORK="testnet"
export NEAR_RPC_URL="https://rpc.testnet.near.org"
export NEAR_CONTRACT_ID="calimero-context-config.testnet"
export NEAR_ACCOUNT_ID="<PUT_YOUR_ACCOUNT_ID_HERE>"
export NEAR_PUBLIC_KEY="<PUT_YOUR_PUBLIC_KEY_HERE>"
export NEAR_SECRET_KEY="<PUT_YOUR_SECRET_KEY_HERE>"

# Starknet Protocol Configuration
export ENABLE_STARKNET=true
export STARKNET_NETWORK="sepolia"
export STARKNET_RPC_URL="https://free-rpc.nethermind.io/sepolia-juno/"
export STARKNET_CONTRACT_ID="0x1b991ee006e2d1e372ab96d0a957401fa200358f317b681df2948f30e17c29c"
export STARKNET_ACCOUNT_ID="<PUT_YOUR_ACCOUNT_ID_HERE>"
export STARKNET_PUBLIC_KEY="<PUT_YOUR_PUBLIC_KEY_HERE>"
export STARKNET_SECRET_KEY="<PUT_YOUR_SECRET_KEY_HERE>"

# ICP Protocol Configuration
export ENABLE_ICP=true
export ICP_NETWORK="local"
export ICP_RPC_URL="http://127.0.0.1:4943"
export ICP_CONTRACT_ID="bkyz2-fmaaa-aaaaa-qaaaq-cai"
export ICP_ACCOUNT_ID="<PUT_YOUR_ACCOUNT_ID_HERE>"
export ICP_PUBLIC_KEY="<PUT_YOUR_PUBLIC_KEY_HERE>"
export ICP_SECRET_KEY="<PUT_YOUR_SECRET_KEY_HERE>"

# Ethereum Protocol Configuration
export ENABLE_ETHEREUM=true
export ETHEREUM_NETWORK="sepolia"
export ETHEREUM_RPC_URL="https://sepolia.drpc.org"
export ETHEREUM_CONTRACT_ID="0x83365DE41E1247511F4C5D10Fb1AFe59b96aD4dB"
export ETHEREUM_ACCOUNT_ID="<PUT_YOUR_ACCOUNT_ID_HERE>"
export ETHEREUM_SECRET_KEY="<PUT_YOUR_SECRET_KEY_HERE>"
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
account_id = "<PUT_YOUR_ACCOUNT_ID_HERE>"
public_key = "<PUT_YOUR_PUBLIC_KEY_HERE>"
secret_key = "<PUT_YOUR_SECRET_KEY_HERE>"

[protocols.starknet]
network = "sepolia"
rpc_url = "https://free-rpc.nethermind.io/sepolia-juno/"
contract_id = "<PUT_CONTRACT_ID_HERE>"
# ... credentials
```

Then run:
```bash
mero-relayer --config relayer-config.toml
```

### Migration from Merod

The relayer functionality has been **completely removed from merod** and is now only available as a standalone service. The relayer no longer depends on `merod` configuration.

```bash
# OLD (no longer available):
# merod --node-name my-node relay --listen 127.0.0.1:63529

# NEW (standalone with own config):
mero-relayer --listen 127.0.0.1:63529
```


## Environment Variables

### General Settings
- `RELAYER_LISTEN_URL`: Listen address (default: "0.0.0.0:63529")
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
NEAR_ACCOUNT_ID=<PUT_YOUR_ACCOUNT_ID_HERE>
NEAR_PUBLIC_KEY=<PUT_YOUR_PUBLIC_KEY_HERE>
NEAR_SECRET_KEY=<PUT_YOUR_SECRET_KEY_HERE>

# Ethereum
ENABLE_ETHEREUM=true
ETHEREUM_ACCOUNT_ID=<PUT_YOUR_ACCOUNT_ID_HERE>
ETHEREUM_SECRET_KEY=<PUT_YOUR_SECRET_KEY_HERE>
```

## API

The relayer exposes a single HTTP POST endpoint at `/` that accepts JSON-RPC 2.0 requests for executing context configuration operations.

### Request Format

The relayer accepts JSON-RPC 2.0 requests with the following structure:

```json
{
  "jsonrpc": "2.0",
  "id": "request-id",
  "method": "execute",
  "params": {
    "contextId": "context-id",
    "method": "method-name",
    "argsJson": {},
    "executorPublicKey": "public-key",
    "substitute": []
  }
}
```

**Request Fields:**
- `jsonrpc`: Must be `"2.0"` (JSON-RPC version)
- `id`: Request identifier (string, number, or null)
- `method`: Must be `"execute"` for context operations
- `params.contextId`: The context ID to operate on
- `params.method`: The specific method to call on the context
- `params.argsJson`: JSON arguments for the method call
- `params.executorPublicKey`: Public key of the executor
- `params.substitute`: Optional array of public key aliases for substitution

### Response Format

The relayer returns JSON-RPC 2.0 responses:

```json
{
  "jsonrpc": "2.0",
  "id": "request-id",
  "result": {
    "output": "method-result"
  }
}
```

For errors:
```json
{
  "jsonrpc": "2.0",
  "id": "request-id",
  "error": {
    "code": -32600,
    "message": "Invalid Request"
  }
}
```

### Example Usage

```bash
# Execute a method on a context
curl -X POST http://localhost:63529/ \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": "1",
    "method": "execute",
    "params": {
      "contextId": "my-context",
      "method": "get_application",
      "argsJson": {},
      "executorPublicKey": "ed25519:...",
      "substitute": []
    }
  }'
```

For more details on the JSON-RPC format and available methods, see the [Calimero Context Configuration documentation](https://github.com/calimero-network/core/tree/master/crates/context/config).

## Architecture

The relayer is a self-contained binary that:

1. **Loads Configuration**: From environment variables or config file
2. **Initializes Blockchain Clients**: Creates transport clients for enabled protocols
3. **Runs HTTP Server**: Forwards incoming requests to appropriate blockchain transports
4. **Operates Independently**: No dependency on `merod` node configuration

The relayer builds its own `calimero-context-config::client::Client` from its standalone configuration and handles all blockchain interactions directly.
