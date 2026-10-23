# Calimero Core

**Ship the app. Skip the infrastructure.**

Calimero is a confidential application platform. You write an app once and it runs in a managed confidential cloud, on your own nodes, or peer-to-peer, with state that syncs between nodes on its own.
This repository is the open-source runtime under all three: the node, the SDK and the protocol.

- **Control without hosting.** Customers keep their data on their terms; nobody runs a separate on-prem edition.
- **One signed bundle.** App logic (WASM) and its frontend ship as one signed package, and the same build runs on every node.
- **Shared apps, no shared server.** Several parties run one app over replicated state that converges with CRDTs, with no consensus and no central database; groups decide who reads and writes what.
- **Don't trust us. Verify us.** Nodes can run sealed in Intel TDX confidential VMs that attest the image they boot; keys are released only to nodes whose measurements match the release policy (see [mero-tee](https://github.com/calimero-network/mero-tee)).

> **Full documentation**: <https://calimero-network.github.io/core/> — organized into four tracks: **Build** (write apps), **Operate** (run nodes), **Protocol Reference** (reimplement / understand the model), and **Contribute** (work on core).

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

| I want to… | Start here |
|-------|------|
| Write a Calimero app | [Build](https://calimero-network.github.io/core/build/) — quickstart, SDK, collections, examples |
| Run and configure a node | [Operate](https://calimero-network.github.io/core/operate/) — install, `merod`/`meroctl`, config, admin API |
| Reimplement a node / understand the model | [Protocol Reference](https://calimero-network.github.io/core/protocol/overview/) — concepts, the operation DAG, sync, the spec |
| Work on Calimero Core itself | [Contribute](https://calimero-network.github.io/core/contribute/) — architecture, crate guide, dev workflow |
| Look up a term | [Glossary](https://calimero-network.github.io/core/protocol/glossary/) |

The docs live in [`docs/`](docs/) (Astro Starlight).

## Related Repositories

- [calimero-network/merobox](https://github.com/calimero-network/merobox) -- E2E testing framework
- [calimero-network/mero-tee](https://github.com/calimero-network/mero-tee) -- TEE infrastructure (KMS, locked images)
- [calimero-network/cargo-mero](https://github.com/calimero-network/cargo-mero) -- `cargo mero` app toolchain (scaffold, build, test, bundle)
- [calimero-network/mero-js](https://github.com/calimero-network/mero-js) -- JavaScript/TypeScript client
- [calimero-network/calimero-client-py](https://github.com/calimero-network/calimero-client-py) -- Python client

## License

MIT OR Apache-2.0
