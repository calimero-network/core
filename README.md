# Calimero Core

Peer-to-peer application platform with local-first governance, CRDT state sync, and WASM execution.

> **Full documentation**: [Architecture Reference](https://calimero-network.github.io/core/)

## Components

| Crate | Description |
|-------|-------------|
| **calimero-node** | NodeManager actor -- orchestrates network events, sync, blobs |
| **calimero-context** | ContextManager actor -- contexts, groups, governance DAGs |
| **calimero-network** | NetworkManager actor -- libp2p swarm, gossipsub, streams |
| **calimero-store** | Column-family KV abstraction over RocksDB |
| **calimero-runtime** | Wasmer WASM execution engine with 50+ host functions |
| **calimero-server** | Axum HTTP server -- REST, JSON-RPC, WS, SSE |
| **calimero-sdk** | App development SDK with proc macros |
| **mero-auth** | JWT auth service with pluggable providers |
| **calimero-dag** | Generic in-memory causal DAG |
| **calimero-storage** | CRDT collections and storage interface |
| **merod** | Node daemon (init, config, run) |
| **meroctl** | Operator CLI (app, context, group, blob management) |

## Documentation

| Topic | Page |
|-------|------|
| System overview and crate map | [System Overview](https://calimero-network.github.io/core/system-overview.html) |
| Local governance and groups | [Local Governance](https://calimero-network.github.io/core/local-governance.html) |
| App signing, bundles, migrations | [App Lifecycle](https://calimero-network.github.io/core/app-lifecycle.html) |
| Wire protocol and sync | [Wire Protocol](https://calimero-network.github.io/core/wire-protocol.html) |
| Sequence diagrams | [Sequence Diagrams](https://calimero-network.github.io/core/sequence-diagrams.html) |
| Storage schema | [Storage Schema](https://calimero-network.github.io/core/storage-schema.html) |
| Error flows | [Error Flows](https://calimero-network.github.io/core/error-flows.html) |
| Config reference | [Config Reference](https://calimero-network.github.io/core/config-reference.html) |
| TEE mode and KMS | [TEE Mode](https://calimero-network.github.io/core/tee-mode.html) |
| Release process | [Release Process](https://calimero-network.github.io/core/release.html) |
| Glossary | [Glossary](https://calimero-network.github.io/core/glossary.html) |

## Related Repositories

- [calimero-network/merobox](https://github.com/calimero-network/merobox) -- E2E testing framework
- [calimero-network/mero-tee](https://github.com/calimero-network/mero-tee) -- TEE infrastructure (KMS, locked images)
- [calimero-network/calimero-client-js](https://github.com/calimero-network/calimero-client-js) -- JavaScript client
- [calimero-network/calimero-client-py](https://github.com/calimero-network/calimero-client-py) -- Python client

## License

MIT OR Apache-2.0
