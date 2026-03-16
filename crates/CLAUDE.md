# crates/ - Core Library Crates

Core library and binary crates for Calimero infrastructure.

## Binary Crates

| Crate | Binary | Entry | Purpose |
|---|---|---|---|
| `merod` | `merod` | `merod/src/main.rs` | Node daemon |
| `meroctl` | `meroctl` | `meroctl/src/main.rs` | CLI tool |
| `relayer` | `mero-relayer` | `relayer/src/main.rs` | Blockchain relay |
| `auth` | `mero-auth` | `auth/src/main.rs` | Auth service |

## Library Crates

| Crate | Entry | Purpose |
|---|---|---|
| `calimero-node` | `node/src/lib.rs` | Node runtime coordination |
| `calimero-runtime` | `runtime/src/lib.rs` | WASM execution (wasmer) |
| `calimero-storage` | `storage/src/lib.rs` | CRDT collections |
| `calimero-network` | `network/src/lib.rs` | P2P networking (libp2p) |
| `calimero-server` | `server/src/lib.rs` | HTTP/WS/SSE server |
| `calimero-context` | `context/src/lib.rs` | Context lifecycle |
| `calimero-dag` | `dag/src/lib.rs` | DAG causal ordering |
| `calimero-store` | `store/src/lib.rs` | KV store (RocksDB) |
| `calimero-sdk` | `sdk/src/lib.rs` | App development SDK |

## Support Crates

| Crate | Purpose |
|---|---|
| `calimero-primitives` | Shared types: `ContextId`, `ApplicationId`, `PublicKey`, `Hash` |
| `calimero-crypto` | Cryptographic utilities |
| `calimero-config` | Configuration parsing |
| `calimero-client` | HTTP/WS client for nodes |

## Structural Patterns

### Primitives Sub-crates

Shared types go in `*-primitives` crates to avoid circular deps:

```
context/primitives/   → calimero-context-primitives
node/primitives/      → calimero-node-primitives
network/primitives/   → calimero-network-primitives
server/primitives/    → calimero-server-primitives
```

### Actix Actors

Node components use actix for async coordination:
- Actor definitions: `node/src/lib.rs`
- Handler pattern: `node/src/handlers/network_event.rs`

### Common Dependencies

```toml
eyre     = "0.6"      # Error handling
tokio    = "1.47"     # Async runtime
actix    = "0.13"     # Actor framework
borsh    = "1.3"      # Binary serialization (storage)
serde    = "1.0"      # JSON serialization (API)
libp2p   = "0.56"     # P2P networking
wasmer   = "6.1"      # WASM runtime
rocksdb  = "0.22"     # Storage backend
```

## Build Commands

```bash
cargo build -p calimero-node
cargo test -p calimero-node
cargo test -p calimero-dag test_dag_out_of_order -- --nocapture
cargo test -p calimero-sdk-macros
```

## Quick Search

```bash
rg -n "pub fn " runtime/src/logic/host_functions/
rg -n "pub async fn handle" node/src/handlers/
rg -n "pub async fn " server/src/admin/
rg -n "pub struct" primitives/src/
rg -n "pub trait" storage/src/
```

## Sub-package CLAUDE.md

- [merod/CLAUDE.md](merod/CLAUDE.md) — Node daemon
- [meroctl/CLAUDE.md](meroctl/CLAUDE.md) — CLI tool
- [node/CLAUDE.md](node/CLAUDE.md) — Node orchestration
- [runtime/CLAUDE.md](runtime/CLAUDE.md) — WASM runtime
- [storage/CLAUDE.md](storage/CLAUDE.md) — CRDT storage
- [sdk/CLAUDE.md](sdk/CLAUDE.md) — App SDK
- [server/CLAUDE.md](server/CLAUDE.md) — HTTP/WS server
- [network/CLAUDE.md](network/CLAUDE.md) — P2P networking
