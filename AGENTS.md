# Calimero Core - AI Agent Guidance

Decentralized infrastructure platform with WebAssembly runtime, P2P networking, and blockchain integrations.

- **Type**: Rust monorepo (Cargo workspace)
- **Stack**: Rust 1.88.0, wasmer (WASM), libp2p (P2P), RocksDB
- **Sub-package AGENTS.md**: See [crates/](crates/AGENTS.md), [apps/](apps/AGENTS.md), [tools/](tools/AGENTS.md)

## Setup Commands

```bash
# Install dependencies & build
cargo build

# Build all (release)
cargo build --release

# Typecheck all
cargo check --workspace

# Test all
cargo test

# Format check
cargo fmt --check

# Lint
cargo clippy -- -A warnings
```

## Universal Conventions

### Import Organization (StdExternalCrate Pattern)

```rust
// 1. Standard library
use std::collections::HashMap;
use std::sync::Arc;

// 2. External crates
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

// 3. Local crate & parent module
use crate::{common, Node};
use super::Shared;

// 4. Local modules
mod config;
mod types;
```

### Module Organization

Do NOT use `mod.rs`. Export modules from named files:

```
crates/meroctl/src/cli/app.rs       # Contains: mod get; mod install;
crates/meroctl/src/cli/app/get.rs
crates/meroctl/src/cli/app/install.rs
```

**Exceptions:** Rare exceptions exist for specific technical reasons (e.g., `crates/node/src/sync/mod.rs` - see [crates/node/AGENTS.md](crates/node/AGENTS.md)). New `mod.rs` files should only be created with explicit justification and documentation of the exception.

### Error Handling

- Use `eyre` crate: `use eyre::Result as EyreResult;`
- Avoid `.unwrap()` / `.expect()` - use `.map_err()` or `?`
- Comment if unwrap is safe: `// SAFETY: guaranteed by X`

### No Dead Code

- **All code in PRs must be used** - no unused functions, variables, imports, or types
- Remove commented-out code blocks before submitting
- If code is for future use, don't include it yet - add it when needed
- Use `#[allow(dead_code)]` only with a comment explaining why (e.g., FFI, test fixtures)
- For detecting and removing dead code: use the **dead-code-cleanup** skill (`.cursor/skills/dead-code-cleanup/SKILL.md`) – it verifies no references before removal and produces a structured report

### Commit Format

```
<type>(<scope>): <short summary>
```

Types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`, `build`, `ci`, `style`, `revert`

- Imperative present tense ("add" not "added")
- No period, no capitalization

## Security & Secrets

- **NEVER** commit tokens, keys, or credentials
- Secrets: `~/.calimero/node/config.toml` (local only)
- No `.env` files in repo

## JIT Index (what to open, not what to paste)

### Package Structure

| Directory         | Purpose                 | AGENTS.md                                            |
| ----------------- | ----------------------- | ---------------------------------------------------- |
| `crates/`         | Core library crates     | [crates/AGENTS.md](crates/AGENTS.md)                 |
| `crates/merod/`   | Node daemon binary      | [crates/merod/AGENTS.md](crates/merod/AGENTS.md)     |
| `crates/meroctl/` | CLI tool                | [crates/meroctl/AGENTS.md](crates/meroctl/AGENTS.md) |
| `crates/node/`    | Node orchestration      | [crates/node/AGENTS.md](crates/node/AGENTS.md)       |
| `crates/runtime/` | WASM execution (wasmer) | [crates/runtime/AGENTS.md](crates/runtime/AGENTS.md) |
| `crates/storage/` | CRDT collections        | [crates/storage/AGENTS.md](crates/storage/AGENTS.md) |
| `crates/sdk/`     | App development SDK     | [crates/sdk/AGENTS.md](crates/sdk/AGENTS.md)         |
| `crates/server/`  | HTTP/WS/SSE server      | [crates/server/AGENTS.md](crates/server/AGENTS.md)   |
| `crates/network/` | P2P networking (libp2p) | [crates/network/AGENTS.md](crates/network/AGENTS.md) |
| `apps/`           | Example WASM apps       | [apps/AGENTS.md](apps/AGENTS.md)                     |
| `tools/`          | Dev tools (merodb, abi) | [tools/AGENTS.md](tools/AGENTS.md)                   |

### Quick Find Commands

```bash
# Search for a function across crates
rg -n "fn function_name" crates/

# Find a struct definition
rg -n "pub struct StructName" crates/

# Find trait implementations
rg -n "impl.*TraitName.*for" crates/

# Find tests for a module
rg -n "#\[test\]" crates/module_name/

# Find all entry points (main.rs)
rg -l "fn main" crates/*/src/

# Find host functions (WASM imports)
rg -n "fn calimero_" crates/runtime/src/
```

## Definition of Done

Before creating a PR:

1. `cargo fmt --check` passes
2. `cargo clippy -- -A warnings` passes
3. `cargo test` passes
4. `cargo deny check licenses sources` passes (if modifying dependencies)
5. **Update relevant documentation** at the end of changes – README, AGENTS.md, crate docs, or API docs as needed; docs must be updated no later than one day after merge

## Data Flow Overview

```
Client Request → JSON-RPC Server → WASM Runtime → Storage (CRDTs)
                                        ↓
                             State Delta → DAG → Network (Gossipsub)
                                        ↓
                             Other Nodes receive & apply delta
```

## Core Concepts (Summary)

- **Context**: Application instance with shared synchronized state (32-byte `ContextId`)
- **CRDTs**: Automatic conflict resolution (`Counter`, `LwwRegister<T>`, `UnorderedMap<K,V>`, `Vector<T>`)
- **DAG**: Causal ordering of state changes with parent references
- **Gossipsub**: P2P message broadcasting per context topic

## Running Local Nodes

```bash
# Initialize and run first node
merod --node node1 init --server-port 2428 --swarm-port 2528
merod --node node1 run

# Second node connecting to first
merod --node node2 init --server-port 2429 --swarm-port 2529
merod --node node2 config --swarm-addrs /ip4/127.0.0.1/tcp/2528
merod --node node2 run

# Debug logging
RUST_LOG=debug merod --node node1 run
```

## Building WASM Apps

```bash
# Add WASM target
rustup target add wasm32-unknown-unknown

# Build specific app
cargo build -p kv-store --target wasm32-unknown-unknown --release

# Build all apps
./scripts/build-all-apps.sh
```
