# merod - Node Daemon

The Calimero node daemon that orchestrates WASM apps, storage, networking, and RPC.

## Package Identity

- **Binary**: `merod`
- **Entry**: `src/main.rs`
- **Framework**: clap (CLI), tokio (async), actix (actors)

## Commands

```bash
# Build
cargo build -p merod

# Build release
cargo build -p merod --release

# Run
cargo run -p merod -- --node node1 run

# Test (no specific tests, integration via node crate)
cargo test -p calimero-node
```

## CLI Structure

```
merod --node <name> <subcommand>
├── init          # Initialize node configuration
├── run           # Start the node daemon
├── config        # Modify node configuration
└── version       # Show version info
```

## File Organization

```
src/
├── main.rs           # Entry point, setup tracing
├── cli.rs            # Root clap command
├── cli/
│   ├── init.rs       # Node initialization
│   ├── run.rs        # Start daemon
│   ├── config.rs     # Config modifications
│   └── auth_mode.rs  # Authentication mode handling
├── defaults.rs       # Default values
├── docker.rs         # Docker integration
├── kms.rs            # Key management service
└── version.rs        # Version checking
```

## Patterns

### CLI Command Pattern

- ✅ DO: Follow pattern in `src/cli/init.rs`
- ✅ DO: Use `EyreResult` for error handling
- ❌ DON'T: Use `unwrap()` or `expect()` without safety comment

```rust
// Pattern: src/cli/init.rs
use clap::Parser;
use eyre::Result as EyreResult;

#[derive(Debug, Parser)]
pub struct InitCommand {
    #[clap(long)]
    server_port: Option<u16>,
}

impl InitCommand {
    pub async fn run(self, args: &RootArgs) -> EyreResult<()> {
        // ...
    }
}
```

### Logging Setup

```rust
// src/main.rs pattern
use tracing_subscriber::fmt::layer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{registry, EnvFilter};

// Default: merod=info,calimero_=info
// Override with RUST_LOG env var
```

## Key Files

| File              | Purpose                   |
| ----------------- | ------------------------- |
| `src/main.rs`     | Entry, tracing setup      |
| `src/cli.rs`      | Root command definition   |
| `src/cli/run.rs`  | Main daemon startup logic |
| `src/cli/init.rs` | Node initialization       |
| `src/defaults.rs` | Default ports, paths      |

## JIT Index

```bash
# Find CLI subcommands
rg -n "#\[derive.*Parser\]" src/

# Find default values
rg -n "const " src/defaults.rs

# Find error handling
rg -n "EyreResult" src/
```

## Running

```bash
# Initialize node
merod --node node1 init --server-port 2428 --swarm-port 2528

# Run with debug logging
RUST_LOG=debug merod --node node1 run

# Run with specific crate logging
RUST_LOG=calimero_node=debug,calimero_network=debug merod --node node1 run
```

## Common Gotchas

- Node data stored at `~/.calimero/<node-name>/`
- Config file: `~/.calimero/<node-name>/config.toml`
- Ports must be available before starting
