# Calimero Relayer

The `calimero-relayer` crate provides a relay server for external client interactions with the Calimero network. It forwards requests to the appropriate blockchain protocols based on the configuration.

## Usage

### As a Library

You can use the relayer as a library in your Rust applications:

```rust
use calimero_relayer::{RelayerConfig, RelayerService};
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let listen_addr = "127.0.0.1:63529".parse::<SocketAddr>()?;
    let node_path = "/path/to/node/config".into();
    
    let config = RelayerConfig::new(listen_addr, node_path);
    let service = RelayerService::new(config);
    
    service.start().await
}
```

### As a Standalone Binary

The crate also provides a standalone binary that can be run independently:

```bash
# Build the binary
cargo build --bin calimero-relayer

# Run with default settings (listens on 0.0.0.0:63529)
./target/debug/calimero-relayer --node-name my-node

# Run with custom address
./target/debug/calimero-relayer --listen 127.0.0.1:8080 --node-name my-node

# Run with custom home directory
./target/debug/calimero-relayer --home /custom/path --node-name my-node
```

### Migration from Merod

The relayer functionality has been **moved out of merod** and is now only available as a standalone service. If you were previously using `merod relay`, please use the standalone binary instead:

```bash
# OLD (no longer available):
# merod --node-name my-node relay --listen 127.0.0.1:63529

# NEW (standalone):
calimero-relayer --node-name my-node --listen 127.0.0.1:63529
```

## Configuration

The relayer requires a properly configured Calimero node. The node configuration should include:

- Client configuration for supported protocols (Near, Starknet, ICP, Ethereum)
- Network configurations and RPC endpoints
- Signing credentials (local or relayer-based)

## Environment Variables

- `CALIMERO_HOME`: Directory for config and data (default: `~/.calimero`)
- `PORT`: Override the port from command line arguments
- `RUST_LOG`: Set logging level (e.g., `RUST_LOG=info`)

## API

The relayer exposes a single HTTP POST endpoint at `/` that accepts JSON requests in the format expected by the Calimero context configuration client.

## Architecture

The relayer consists of:

1. **RelayerConfig**: Configuration structure containing listen address and node path
2. **RelayerService**: Main service that handles HTTP requests and forwards them to blockchain transports
3. **Standalone Binary**: Self-contained executable for running the relayer independently

The relayer loads the node configuration, initializes blockchain client transports, and spawns an HTTP server that forwards incoming requests to the appropriate transport based on the protocol specified in the request.
