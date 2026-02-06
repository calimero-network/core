# Crates Directory - AI Agent Guidance

Core library crates for Calimero infrastructure. Each crate is conceptually separate.

## Crate Categories

### Binary Crates (executables)

| Crate     | Binary         | Entry Point           | Purpose          |
| --------- | -------------- | --------------------- | ---------------- |
| `merod`   | `merod`        | `merod/src/main.rs`   | Node daemon      |
| `meroctl` | `meroctl`      | `meroctl/src/main.rs` | CLI tool         |
| `relayer` | `mero-relayer` | `relayer/src/main.rs` | Blockchain relay |
| `auth`    | `mero-auth`    | `auth/src/main.rs`    | Auth service     |

### Core Library Crates

| Crate              | Entry Point          | Purpose                   |
| ------------------ | -------------------- | ------------------------- |
| `calimero-node`    | `node/src/lib.rs`    | Node runtime coordination |
| `calimero-runtime` | `runtime/src/lib.rs` | WASM execution (wasmer)   |
| `calimero-storage` | `storage/src/lib.rs` | CRDT collections          |
| `calimero-network` | `network/src/lib.rs` | P2P networking (libp2p)   |
| `calimero-server`  | `server/src/lib.rs`  | HTTP/WS/SSE server        |
| `calimero-context` | `context/src/lib.rs` | Context lifecycle         |
| `calimero-dag`     | `dag/src/lib.rs`     | DAG causal ordering       |
| `calimero-store`   | `store/src/lib.rs`   | KV store (RocksDB)        |
| `calimero-sdk`     | `sdk/src/lib.rs`     | App development SDK       |

### Support Crates

| Crate                 | Purpose                                                         |
| --------------------- | --------------------------------------------------------------- |
| `calimero-primitives` | Shared types: `ContextId`, `ApplicationId`, `PublicKey`, `Hash` |
| `calimero-crypto`     | Cryptographic utilities                                         |
| `calimero-config`     | Configuration parsing                                           |
| `calimero-client`     | HTTP/WS client for nodes                                        |

## Patterns & Conventions

### Primitives Crates Pattern

Shared types go in `*-primitives` crates to avoid circular dependencies:

```
context/primitives/  → calimero-context-primitives
node/primitives/     → calimero-node-primitives
network/primitives/  → calimero-network-primitives
server/primitives/   → calimero-server-primitives
```

### Config Crates Pattern

Configuration types often in separate `*-config` crates:

```
context/config/      → calimero-context-config
```

### Actix Actors

Node components use actix actor framework for async coordination:

- ✅ See pattern: `node/src/handlers/network_event.rs`
- ✅ Actor definitions: `node/src/lib.rs`

### File Organization

```rust
// src/lib.rs - exports and top-level types
pub mod handlers;
pub mod sync;
mod constants;
mod utils;

// Each handler in separate file
// src/handlers/network_event.rs
// src/handlers/state_delta.rs
```

## Common Dependencies

```toml
# Error handling
eyre = "0.6"

# Async runtime
tokio = "1.47"
actix = "0.13"

# Serialization
borsh = "1.3"      # Binary (storage)
serde = "1.0"      # JSON (API)

# Networking
libp2p = "0.56"

# WASM
wasmer = "6.1"

# Storage
rocksdb = "0.22"
```

## Commands

```bash
# Build specific crate
cargo build -p calimero-node

# Test specific crate
cargo test -p calimero-node

# Test with output
cargo test -p calimero-dag test_dag_out_of_order -- --nocapture

# Run SDK macro tests (compile-time)
cargo test -p calimero-sdk-macros
```

## JIT Index

### Find Functions

```bash
# Find host functions
rg -n "pub fn " runtime/src/logic/host_functions/

# Find handlers
rg -n "pub async fn handle" node/src/handlers/

# Find API endpoints
rg -n "pub async fn " server/src/admin/
```

### Find Types

```bash
# Find struct definitions
rg -n "pub struct" primitives/src/

# Find enums
rg -n "pub enum" -A5 context/primitives/src/

# Find trait definitions
rg -n "pub trait" storage/src/
```

## Sub-Package AGENTS.md

- [merod/AGENTS.md](merod/AGENTS.md) - Node daemon
- [meroctl/AGENTS.md](meroctl/AGENTS.md) - CLI tool
- [node/AGENTS.md](node/AGENTS.md) - Node orchestration
- [runtime/AGENTS.md](runtime/AGENTS.md) - WASM runtime
- [storage/AGENTS.md](storage/AGENTS.md) - CRDT storage
- [sdk/AGENTS.md](sdk/AGENTS.md) - App SDK
- [server/AGENTS.md](server/AGENTS.md) - HTTP/WS server
- [network/AGENTS.md](network/AGENTS.md) - P2P networking
