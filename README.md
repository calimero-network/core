# Calimero Core

The Calimero confidential cloud: apps written in Rust or JavaScript and compiled to WASM run on attested TEE nodes, one Raft log per app, with passkey identity, organizations and teams.

> **Full documentation**: <https://calimero-network.github.io/core/> — organized into four tracks: **Build** (write apps), **Operate** (run nodes), **Protocol Reference** (understand the model), and **Contribute** (work on core).

## Components

| Crate | Description |
|-------|-------------|
| **calimero-cloud-node** (`merod-v2`) | The cloud node: system group, app groups, sealed API, `merod-v2 local` dev cluster |
| **calimero-cloud-client** (`calimero-cloud`) | Rust client and CLI: attestation, sealed channel, passkey login |
| **calimero-log** | Multi-group Raft log (openraft) |
| **calimero-identity** | Passkeys, sessions, API keys |
| **calimero-sealed** | Attested sealed request channel |
| **calimero-kms-client** | mero-kms client: release-policy-verified storage key fetch |
| **calimero-runtime** | Wasmer WASM execution engine with 50+ host functions |
| **calimero-sdk** | App development SDK with proc macros |
| **calimero-storage** | CRDT collections and storage interface |
| **calimero-search** | Per-app full-text search (tantivy) |

## Documentation

| I want to… | Start here |
|-------|------|
| Write a Calimero app | [Build](https://calimero-network.github.io/core/build/) — quickstart, SDK, collections, examples |
| Run and configure a node | [Operate](https://calimero-network.github.io/core/operate/) — `merod-v2`, its configuration and the Ops API |
| Understand the model | [Protocol Reference](https://calimero-network.github.io/core/protocol/overview/) — concepts and the spec |
| Work on Calimero Core itself | [Contribute](https://calimero-network.github.io/core/contribute/) — architecture, crate guide, dev workflow |
| Look up a term | [Glossary](https://calimero-network.github.io/core/protocol/glossary/) |

The docs live in [`docs/`](docs/) (Astro Starlight).

## Related Repositories

- [calimero-network/merobox](https://github.com/calimero-network/merobox) -- E2E testing framework (`cloud-v2` workflows)
- [calimero-network/mero-tee](https://github.com/calimero-network/mero-tee) -- TEE infrastructure (KMS, locked images)
- [calimero-network/cargo-mero](https://github.com/calimero-network/cargo-mero) -- `cargo mero` app toolchain (scaffold, build, test, bundle)
- [calimero-network/mero-js](https://github.com/calimero-network/mero-js) -- JavaScript/TypeScript client
- [calimero-network/calimero-client-py](https://github.com/calimero-network/calimero-client-py) -- Python client

## License

MIT OR Apache-2.0
