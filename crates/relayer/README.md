# Calimero Relayer

The `mero-relayer` is a standalone relay server for external client interactions with the Calimero network. It forwards requests to the configured NEAR transport and operates independently of the main `merod` node.

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

To customize NEAR network settings or credentials, use environment variables:

```bash
# Basic relayer settings
export RELAYER_LISTEN_URL="0.0.0.0:63529"

# Near Protocol Configuration
export ENABLE_NEAR=true
export NEAR_NETWORK="testnet"
export NEAR_RPC_URL="https://rpc.testnet.near.org"
export NEAR_CONTRACT_ID="v0-6.config.calimero-context.testnet"
export NEAR_ACCOUNT_ID="<PUT_YOUR_ACCOUNT_ID_HERE>"
export NEAR_PUBLIC_KEY="<PUT_YOUR_PUBLIC_KEY_HERE>"
export NEAR_SECRET_KEY="<PUT_YOUR_SECRET_KEY_HERE>"
```

#### 3. Configuration File

Create a TOML or JSON configuration file:

```toml
# relayer-config.toml
listen = "0.0.0.0:63529"

[protocols.near]
network = "testnet"
rpc_url = "https://rpc.testnet.near.org"
contract_id = "v0-6.config.calimero-context.testnet"

[protocols.near.credentials]
type = "near"
account_id = "<PUT_YOUR_ACCOUNT_ID_HERE>"
public_key = "<PUT_YOUR_PUBLIC_KEY_HERE>"
secret_key = "<PUT_YOUR_SECRET_KEY_HERE>"
```

Then run:
```bash
mero-relayer --config relayer-config.toml
```

### Migration from Merod

The relayer functionality has been **completely removed from merod** and is now only available as a standalone service. The relayer no longer depends on `merod` configuration.

```bash
# OLD (no longer available):
# merod --node my-node relay --listen 127.0.0.1:63529

# NEW (standalone with own config):
mero-relayer --listen 127.0.0.1:63529
```


## Environment Variables

### General Settings
- `RELAYER_LISTEN_URL`: Listen address (default: "0.0.0.0:63529")
- `PORT`: Override port (used by `addr_from_str` parser)
- `RUST_LOG`: Set logging level (e.g., `RUST_LOG=info`)

### NEAR Protocol Configuration
Use the following variables for NEAR relayer configuration:

**Basic protocol settings:**
- `ENABLE_NEAR`: Enable/disable NEAR support (true/false)
- `NEAR_NETWORK`: Network name (for example, `testnet` or `mainnet`)
- `NEAR_RPC_URL`: RPC endpoint URL
- `NEAR_CONTRACT_ID`: Contract account ID

**NEAR credentials:**
- `NEAR_ACCOUNT_ID`: Account ID
- `NEAR_PUBLIC_KEY`: Public key
- `NEAR_SECRET_KEY`: Secret key

**Example:**
```bash
ENABLE_NEAR=true
NEAR_ACCOUNT_ID=<PUT_YOUR_ACCOUNT_ID_HERE>
NEAR_PUBLIC_KEY=<PUT_YOUR_PUBLIC_KEY_HERE>
NEAR_SECRET_KEY=<PUT_YOUR_SECRET_KEY_HERE>
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
