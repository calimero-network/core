# Calimero Core

A confidential cloud for collaborative apps. Apps are WASM, written with the
Calimero SDK; each app instance is replicated by its own Raft log across
attested Intel TDX nodes, reached through a sealed channel the client opens only
after verifying the node's attestation, and signed into with passkeys. Nobody
operating the nodes can read the apps' data.

> **Full documentation**: <https://calimero-network.github.io/core/>, organized into **Cloud** (start here), **Build** (write apps), **Operate** (run a cluster), **Protocol Reference** and **Contribute** (work on core).

## Components

| Crate | Description |
|-------|-------------|
| **merod** | The node: `init`, `run`, `config`, `local` (a development cluster), `inspect`, `kms disk-key` |
| **meroctl** | The command line: apps, contexts, organizations, teams, blobs, accounts, and `ops` for the operator |
| **calimero-node** | The node's inside: system group, app groups, the HTTP and Ops API |
| **calimero-log** | Multi-group Raft (openraft) over a sealed RocksDB store |
| **calimero-sealed** | The sealed HTTP transport, server and client |
| **calimero-identity** | Passkeys, sessions, API keys, confirmation challenges |
| **calimero-client** | Rust client: attestation check, sealed requests, sign-in |
| **calimero-runtime** | Wasmer WASM runtime with gas metering and host functions |
| **calimero-sdk** | App development SDK with proc macros |
| **calimero-storage** | CRDT collections and the storage interface apps write through |

The full list is in [`crates/AGENTS.md`](crates/AGENTS.md).

## Quick start

```bash
cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release
cargo run -p merod -- local                     # 3 nodes on 127.0.0.1:2501-2503
cargo install --path crates/meroctl
meroctl node add local http://127.0.0.1:2501/ --allow-mock
meroctl login --dev-passkey ./dev-passkey.json --name ada
```

Nodes run on Linux; on macOS, run `merod` in Docker and `meroctl` natively.
See [Install & first run](https://calimero-network.github.io/core/operate/install/).

## Documentation

| I want to… | Start here |
|-------|------|
| Understand release-v2 and try it | [Cloud](https://calimero-network.github.io/core/cloud/overview/) — overview, quickstart, writing apps |
| Write a Calimero app | [Build](https://calimero-network.github.io/core/build/) — SDK, collections, examples |
| Run a cluster | [Operate](https://calimero-network.github.io/core/operate/) — install, deployment, `merod`/`meroctl`, configuration, the node API |
| Understand the model | [Protocol Reference](https://calimero-network.github.io/core/protocol/overview/) |
| Work on Calimero Core itself | [Contribute](https://calimero-network.github.io/core/contribute/) — architecture, crate guide, development, testing |

The docs live in [`docs/`](docs/) (Astro Starlight).

## Related Repositories

- [calimero-network/merobox](https://github.com/calimero-network/merobox) -- scenario testing (`cloud-v2` backend)
- [calimero-network/mero-tee](https://github.com/calimero-network/mero-tee) -- TEE images, mero-kms, signed releases
- [calimero-network/mero-js](https://github.com/calimero-network/mero-js) -- JavaScript/TypeScript client
- [calimero-network/calimero-client-py](https://github.com/calimero-network/calimero-client-py) -- Python client

`cargo mero`, the app toolchain, is in [`tools/cargo-mero`](tools/cargo-mero).

## License

MIT OR Apache-2.0
