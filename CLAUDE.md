# Calimero Core

Decentralized infrastructure platform with WebAssembly runtime, P2P networking, and blockchain integrations.

- **Type**: Rust monorepo (Cargo workspace)
- **Stack**: Rust 1.88.0, wasmer (WASM), libp2p (P2P), RocksDB
- **Sub-package CLAUDE.md**: See [crates/](crates/CLAUDE.md), [apps/](apps/CLAUDE.md), [tools/](tools/CLAUDE.md)

## Build & Test

```bash
cargo build                              # Build all
cargo build --release                    # Release build
cargo check --workspace                  # Typecheck only
cargo test                               # Run all tests
cargo test -p <crate-name>               # Test specific crate
cargo fmt --check                        # Format check
cargo clippy -- -A warnings              # Lint
cargo deny check licenses sources        # Dependency audit
```

## Definition of Done

Before any PR:
1. `cargo fmt --check` passes
2. `cargo clippy -- -A warnings` passes
3. `cargo test` passes
4. `cargo deny check licenses sources` passes (if deps changed)
5. Relevant docs updated (README, CLAUDE.md, crate docs)

## Universal Conventions

### Import Order (StdExternalCrate)

```rust
use std::collections::HashMap;   // 1. std
use std::sync::Arc;

use serde::{Deserialize, Serialize}; // 2. external crates
use tokio::sync::RwLock;

use crate::{common, Node};        // 3. local crate & parent
use super::Shared;

mod config;                       // 4. local module definitions
mod types;
```

- One import per `use` line — no grouping within a crate
- Sort `Cargo.toml` dependencies alphabetically

### Module Organization

Do **not** use `mod.rs`. Use named files:

```
crates/meroctl/src/cli/app.rs       ← declares: mod get; mod install;
crates/meroctl/src/cli/app/get.rs
crates/meroctl/src/cli/app/install.rs
```

Exception: `crates/node/src/sync/mod.rs` (documented technical reason).

### Error Handling

```rust
use eyre::Result as EyreResult;
```
- No `.unwrap()` / `.expect()` without a `// SAFETY: ...` comment
- Use `.map_err()` for error mapping
- Short-circuit early: `if !condition { return Err(...); }`
- Prefer `let..else` over deep `if let..else` chains

### Naming

| Thing | Convention |
|---|---|
| Types, enum variants | `PascalCase` |
| Functions, methods, fields, variables | `snake_case` |
| Macros | `snake_case` |
| Constants / statics | `SCREAMING_SNAKE_CASE` |

### No Dead Code

All code in PRs must be used. No unused imports, functions, or types. Use `#[allow(dead_code)]` only with an explanatory comment.

### Commit Format

```
<type>(<scope>): <short summary>
```
Types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`, `build`, `ci`, `style`, `revert`

## Package Map

| Directory | Purpose | CLAUDE.md |
|---|---|---|
| `crates/` | Core library crates | [crates/CLAUDE.md](crates/CLAUDE.md) |
| `crates/merod/` | Node daemon binary | [crates/merod/CLAUDE.md](crates/merod/CLAUDE.md) |
| `crates/meroctl/` | CLI tool | [crates/meroctl/CLAUDE.md](crates/meroctl/CLAUDE.md) |
| `crates/node/` | Node orchestration | [crates/node/CLAUDE.md](crates/node/CLAUDE.md) |
| `crates/runtime/` | WASM execution (wasmer) | [crates/runtime/CLAUDE.md](crates/runtime/CLAUDE.md) |
| `crates/storage/` | CRDT collections | [crates/storage/CLAUDE.md](crates/storage/CLAUDE.md) |
| `crates/sdk/` | App development SDK | [crates/sdk/CLAUDE.md](crates/sdk/CLAUDE.md) |
| `crates/server/` | HTTP/WS/SSE server | [crates/server/CLAUDE.md](crates/server/CLAUDE.md) |
| `crates/network/` | P2P networking (libp2p) | [crates/network/CLAUDE.md](crates/network/CLAUDE.md) |
| `apps/` | Example WASM apps | [apps/CLAUDE.md](apps/CLAUDE.md) |
| `tools/` | Dev tools (merodb, abi) | [tools/CLAUDE.md](tools/CLAUDE.md) |

## Architecture: Data Flow

```
Client Request → JSON-RPC Server → WASM Runtime → Storage (CRDTs)
                                        ↓
                             State Delta → DAG → Network (Gossipsub)
                                        ↓
                             Other Nodes receive & apply delta
```

## Core Concepts

- **Context**: Application instance with shared synchronized state (32-byte `ContextId`)
- **CRDTs**: Conflict-free types — `Counter`, `LwwRegister<T>`, `UnorderedMap<K,V>`, `Vector<T>`
- **DAG**: Causal ordering of state changes with parent hash references
- **Gossipsub**: P2P pub/sub per context topic — all members receive deltas

## Quick Search

```bash
rg -n "fn function_name" crates/
rg -n "pub struct StructName" crates/
rg -n "impl.*TraitName.*for" crates/
rg -n "pub fn " crates/runtime/src/logic/host_functions/
rg -l "fn main" crates/*/src/
```

## Running Nodes Locally

```bash
merod --node node1 init --server-port 2428 --swarm-port 2528
merod --node node1 run

merod --node node2 init --server-port 2429 --swarm-port 2529
merod --node node2 config --swarm-addrs /ip4/127.0.0.1/tcp/2528
merod --node node2 run

RUST_LOG=debug merod --node node1 run
```

Node data lives at `~/.calimero/<node-name>/`.

## Building WASM Apps

```bash
rustup target add wasm32-unknown-unknown
cargo build -p kv-store --target wasm32-unknown-unknown --release
./scripts/build-all-apps.sh
```

## Security

- **Never** commit tokens, keys, or credentials
- Secrets stay in `~/.calimero/<node>/config.toml` (local only)
- No `.env` files in repo
