# Calimero Core

Peer-to-peer application platform with local-first governance, CRDT state sync, and WASM execution.

## Architecture

Interactive architecture reference with system diagrams, crate deep-dives, and data flow documentation:

**[View Architecture Site](https://calimero-network.github.io/core/)**

For local browsing, open `architecture/index.html` in a browser.

## Quick Links

| Resource | Path |
|----------|------|
| Architecture site | [`architecture/`](architecture/) |
| Context management docs | [`docs/context-management/`](docs/context-management/) |
| App lifecycle docs | [`docs/app-lifecycle/`](docs/app-lifecycle/) |
| Release process | [`docs/RELEASE.md`](docs/RELEASE.md) |
| Sample apps | [`apps/`](apps/) |

## Crates

| Crate | Description |
|-------|-------------|
| `calimero-node` | NodeManager actor — orchestrates network events, sync, blobs |
| `calimero-context` | ContextManager actor — contexts, groups, governance DAGs |
| `calimero-network` | NetworkManager actor — libp2p swarm, gossipsub, streams |
| `calimero-store` | Column-family KV abstraction over RocksDB |
| `calimero-runtime` | Wasmer WASM execution engine with 50+ host functions |
| `calimero-server` | Axum HTTP server — REST, JSON-RPC, WS, SSE |
| `calimero-sdk` | App development SDK with proc macros |
| `mero-auth` | JWT auth service with pluggable providers |
| `calimero-dag` | Generic in-memory causal DAG |
| `calimero-storage` | CRDT collections and storage interface |
| `merod` | Node daemon (init, config, run) |
| `meroctl` | Operator CLI (app, context, group, blob management) |

## License

MIT OR Apache-2.0
