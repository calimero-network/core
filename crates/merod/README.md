# merod

Calimero node runtime for running, initializing, and configuring Calimero nodes. This is the main binary that orchestrates WASM applications, storage, networking, and RPC services.

## Installation

```bash
# From source (requires Rust)
cargo install --path core/crates/merod

# Or build directly
cd core/crates/merod
cargo build --release
```

## Quick Start

```bash
# Initialize a node with default settings
merod --node node1 init

# Run the node
merod --node node1 run
```

## Commands

### Initialize Node (`init`)

Initialize a new node configuration:

```bash
# Basic initialization
merod --node node1 init

# Custom ports
merod --node node1 init \
  --server-port 2428 \
  --swarm-port 2528

# Custom home directory
mkdir data
merod --home data/ --node node1 init

# With bootstrap nodes
merod --node node1 init \
  --boot-nodes /ip4/127.0.0.1/tcp/2528

# With protocol configuration (NEAR, Ethereum, ICP, Starknet)
merod --node node1 init \
  --protocol near \
  --relayer-url https://relayer.near.org

# Authentication mode
merod --node node1 init \
  --auth-mode embedded \
  --auth-storage persistent

# Force re-initialization
merod --node node1 init --force
```

**Init Options:**

- `--swarm-host <HOST>` - Host to listen on for P2P (default: `0.0.0.0,::`)
- `--swarm-port <PORT>` - Port for P2P networking (default: `2528`)
- `--server-host <HOST>` - Host for RPC server (default: `127.0.0.1,::1`)
- `--server-port <PORT>` - Port for RPC server (default: `2428`)
- `--boot-nodes <ADDR>...` - Bootstrap nodes for P2P discovery
- `--boot-network <NETWORK>` - Use nodes from known network (`calimero-dev`, `ipfs`)
- `--protocol <PROTOCOL>` - Blockchain protocol (`near`, `ethereum`, `icp`, `starknet`)
- `--relayer-url <URL>` - Relayer URL for blockchain transactions
- `--auth-mode <MODE>` - Authentication mode (`none`, `embedded`, `remote`)
- `--auth-storage <STORAGE>` - Auth storage type (`persistent`, `memory`)
- `--auth-storage-path <PATH>` - Path for persistent auth storage
- `--mdns` - Enable mDNS discovery (default: enabled)
- `--no-mdns` - Disable mDNS discovery
- `--advertise-address` - Advertise observed address
- `--force` - Force initialization even if directory exists

### Configure Node (`config`)

Update configuration of an existing node using TOML key=value pairs. Use the same key paths as in `config.toml` (e.g. `server.listen`, `swarm.listen`, `bootstrap.nodes`). In zsh, quote the argument so `[` and `]` are not globbed.

```bash
# Configure server listen addresses (quote in zsh)
merod --node node1 config "server.listen=['/ip4/0.0.0.0/tcp/3000']"

# Configure swarm listen addresses
merod --node node1 config "swarm.listen=['/ip4/0.0.0.0/tcp/2528']"

# Configure bootstrap nodes
merod --node node1 config "bootstrap.nodes=['/ip4/127.0.0.1/tcp/2528/p2p/PEER_ID']"

# Multiple key=value pairs
merod --node node1 config "server.listen=['/ip4/192.168.1.100/tcp/8080']" "swarm.listen=['/ip4/0.0.0.0/tcp/9090']"
```

Run `merod --help` for more examples.

### Run Node (`run`)

Start and run the configured node:

```bash
# Run with default configuration
merod --node node1 run

# Run with custom home directory
merod --home ~/.calimero-custom --node node1 run

# Override auth mode at runtime
merod --node node1 run --auth-mode embedded
```

## Environment Variables

```bash
# Set default home directory
export CALIMERO_HOME=~/.calimero

# Set NEAR API key for blockchain operations
export NEAR_API_KEY=your_api_key_here

# Configure logging
export RUST_LOG=merod=info,calimero_=info
```

## Node Types

Nodes can operate in two modes:

1. **Coordinator** - First node in a network, creates initial contexts
2. **Peer** - Joins existing network, connects to coordinator or other peers

Both modes are handled automatically based on configuration and network topology.

## Configuration File

Nodes store configuration in `~/.calimero/<node-name>/config.toml`. This includes:

- Network settings (swarm hosts/ports, bootstrap nodes)
- Server settings (RPC host/port)
- Protocol configuration (NEAR, Ethereum, ICP, etc.)
- Authentication settings
- Storage paths

Configuration can be modified via:
- `merod config` command
- Direct TOML file editing (not recommended while node is running)

## Running Multiple Nodes

To run a local multi-node network:

```bash
# Terminal 1: First node (coordinator)
merod --node node1 init --server-port 2428 --swarm-port 2528
merod --node node1 run

# Terminal 2: Second node (peer)
merod --node node2 init --server-port 2429 --swarm-port 2529
merod --node node2 config "bootstrap.nodes=['/ip4/127.0.0.1/tcp/2528/p2p/NODE1_PEER_ID']"
merod --node node2 run

# Terminal 3: Third node (peer)
merod --node node3 init --server-port 2430 --swarm-port 2530
merod --node node3 config "bootstrap.nodes=['/ip4/127.0.0.1/tcp/2528/p2p/NODE1_PEER_ID']"
merod --node node3 run
```

For easier multi-node setup, use [Merobox](../merobox) which automates Docker-based node orchestration.

## Authentication

Nodes support multiple authentication modes:

- **`none`** - No authentication (development only)
- **`embedded`** - Built-in auth server (persistent or memory storage)
- **`remote`** - External auth service URL

See [Authentication Guide](../../crates/auth/README.md) for details.

## Protocol Support

merod supports multiple blockchain protocols:

- **NEAR** - NEAR Protocol integration
- **Ethereum** - Ethereum and EVM-compatible chains
- **ICP** - Internet Computer Protocol
- **Starknet** - Starknet Layer 2

Each protocol requires appropriate relayer configuration for on-chain operations.

## Troubleshooting

```bash
# Check node logs
RUST_LOG=debug merod --node node1 run

# Verify configuration
cat ~/.calimero/node1/config.toml

# Check if ports are available
lsof -i :2428
lsof -i :2528
```

## See Also

- [Calimero Documentation](https://docs.calimero.network) - Complete documentation
- [Running Local Networks](../../docs/operator-track/run-a-local-network.md) - Local development guide
- [Node Architecture](../../crates/node/README.md) - Internal node architecture
- [Network Configuration](../../crates/network/README.md) - P2P networking details
- [Server API](../../crates/server/README.md) - JSON-RPC API reference

