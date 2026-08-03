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

# Test
cargo test -p merod
```

## CLI Structure

```
merod --node <name> <subcommand>
├── account       # Back up / restore the account root (export, import).
│                 # Opens the store directly — the node must be STOPPED.
├── init          # Initialize node configuration (mints the embedded-auth
│                 # admin root key from --admin-user + password via
│                 # file/stdin/env; --no-admin defers)
├── run           # Start the node daemon (alias: up)
├── config        # Modify node configuration
├── auth          # Embedded-auth accounts (set-admin: offline admin-key mint)
└── kms           # Key management service
```

## Account root backup (`merod account`)

The account root is the only key that can certify a replacement device after every
device is lost, so these two commands are the whole recovery story. Both open the
datastore directly, which means the node must be **stopped** (RocksDB's lock is
exclusive) and a KMS-encrypted store is refused rather than misread.

```bash
# Print the 24-word phrase to stdout. Add --namespace (repeatable, optional) to
# also print the account id derived for that namespace.
merod --node node1 account export [--namespace <NAMESPACE_ID>]…

# Write it to a file instead. Refused without the second flag; created 0600.
merod --node node1 account export --out backup.txt --allow-plaintext-file

# Restore. Reads stdin by default, or --from PATH. Refuses to replace an
# existing root without --force.
merod --node node1 account import [--from backup.txt] [--force]
```

`--force` **drops the device rows belonging to the root it replaces**, and reports
which namespaces they were in. Not housekeeping: a device row is keyed by namespace
alone and enrolment refuses to replace a *linked* row naming a different account, so
leaving them made the node refuse enrolment under the root it had just recovered —
telling the operator to revoke first, which needs the key they replaced. Rows naming
an account this root never owned (a device paired into somebody else's account) are
kept and reported separately. An import onto an empty store needs no flag and drops
nothing.

Export prints the phrase on the **first line** (so `head -1` is the secret),
then the root's public key, then any derived account ids. Only the first line is
sensitive; the account ids are what writer sets already name.

Full model, the recovery procedure, and what does *not* come back:
[protocol/accounts](../../docs/src/content/docs/protocol/accounts.mdx#backing-up-and-recovering-an-account).

## File Organization

```
src/
├── main.rs           # Entry point, setup tracing
├── cli.rs            # Root clap command
├── cli/
│   ├── init.rs       # Node initialization
│   ├── run.rs        # Start daemon
│   ├── config.rs     # Config modifications
│   ├── auth.rs       # `merod auth set-admin` (offline admin-key mint)
│   ├── admin_creds.rs# Shared --admin-user/password-file/stdin resolution
│   ├── account.rs    # `merod account export|import` (recovery phrase)
│   ├── kms.rs        # KMS subcommand
│   ├── validation.rs # Validation helpers
│   └── auth_mode.rs  # Authentication mode handling
├── defaults.rs       # Default values
├── kms/              # Key management service
├── kms_policy.rs     # KMS policy
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
