# merod - Node Daemon

The Calimero node daemon that orchestrates WASM apps, storage, networking, and RPC.

- **Binary**: `merod`
- **Entry**: `src/main.rs`
- **Frameworks**: clap (CLI), tokio (async), actix (actors)

## Build & Run

```bash
cargo build -p merod
cargo build -p merod --release
cargo run -p merod -- --node node1 run
cargo test -p calimero-node           # integration tests live in node crate
```

## CLI Structure

```
merod --node <name> <subcommand>
├── init      # Initialize node configuration
├── run       # Start the node daemon
├── config    # Modify node configuration
└── version   # Show version info
```

## File Layout

```
src/
├── main.rs           # Entry point, tracing setup
├── cli.rs            # Root clap command
├── cli/
│   ├── init.rs       # Node initialization
│   ├── run.rs        # Start daemon
│   ├── config.rs     # Config modifications
│   └── auth_mode.rs  # Authentication mode
├── defaults.rs       # Default ports and paths
├── docker.rs         # Docker integration
├── kms.rs            # Key management service
└── version.rs        # Version checking
```

## Patterns

### CLI Command

```rust
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

### Logging

Default filter: `merod=info,calimero_=info`. Override with `RUST_LOG`.

```bash
RUST_LOG=debug merod --node node1 run
RUST_LOG=calimero_node=debug,calimero_network=debug merod --node node1 run
```

## Key Files

| File | Purpose |
|---|---|
| `src/main.rs` | Entry, tracing setup |
| `src/cli.rs` | Root command definition |
| `src/cli/run.rs` | Main daemon startup |
| `src/cli/init.rs` | Node initialization |
| `src/defaults.rs` | Default ports, paths |

## Quick Search

```bash
rg -n "#\[derive.*Parser\]" src/
rg -n "const " src/defaults.rs
rg -n "EyreResult" src/
```

## Node Data

- Config: `~/.calimero/<node-name>/config.toml`
- Data: `~/.calimero/<node-name>/data/`
- Ports must be free before starting
