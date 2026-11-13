# Calimero Core

**Your app's whole backend, like Firebase. Except nobody can read your data, not even us.**

Calimero gives an app what a backend platform does (sign-in, a database,
functions, realtime, storage and hosting) in one deploy, but runs every app
inside attested Intel TDX hardware. The people operating the servers, including
us, can't see the data, and users don't have to take our word for it: their
client verifies the node before it sends anything.

This repository is the platform itself: the node, the command line, the app SDK
and the docs.

> **Full documentation**: <https://calimero-network.github.io/core/>, organized into **Cloud** (start here), **Build** (write apps), **Operate** (run a cluster), **Protocol Reference** and **Contribute** (work on core). Coming from the earlier peer-to-peer release? The [glossary](https://calimero-network.github.io/core/protocol/glossary/) maps its names to these.

## What you get

| You need | Calimero gives you |
| --- | --- |
| Auth | Passkey sign-in, sessions and scoped API keys, built in |
| Database | Replicated app state: maps, sets, tables with indexes, rich text |
| Functions | App methods in Rust, plus compute jobs, timers and pipelines |
| Realtime | Live events and presence streamed to every client |
| Storage | Sealed file and blob storage |
| Search | Full-text search over your app's data |
| Hosting | Your frontend served from the same verified nodes |
| Typed clients | TypeScript, Python, Swift and Kotlin, generated from your app |

## How it works

You write an app in Rust with `calimero-sdk`, build it into a `calimero:app`
WebAssembly component and deploy it as one signed package. Then:

- **Every app instance has its own replicated log.** A Raft group of attested
  nodes (3 by default) holds it. The leader runs each call on wasmtime and the
  followers apply its result.
- **Clients verify before they talk.** A client checks the node's TDX quote
  against signed releases, then opens a sealed (Noise) channel to a key bound
  into that quote. People sign in with passkeys; machines use API keys.
- **Data is sealed at rest.** A node gets its disk key from mero-kms only after
  it attests, so a copied disk or a modified node reads nothing.
- **Every answer is signed.** Results carry a receipt signed by an attested key.
- **Nothing changes without consent.** New code or a new node image reaches an
  app's data only after the app's approver says yes.

It runs on Calimero Cloud, on a dedicated cluster, or on your own TDX hosts.

## Components

| Crate | Description |
|-------|-------------|
| **merod** | The node: `init`, `run`, `config`, `local` (a development cluster), `inspect`, `kms disk-key`, `attest` |
| **meroctl** | The command line: packages, app instances (`context`), organizations, teams, blobs, accounts, and `ops` for the operator |
| **calimero-node** | The node's inside: system group, app groups, the HTTP and Ops API |
| **calimero-log** | Multi-group Raft (openraft) over a sealed RocksDB store |
| **calimero-sealed** | The sealed HTTP transport, server and client |
| **calimero-identity** | Passkeys, sessions, API keys, confirmation challenges |
| **calimero-client** | Rust client: attestation check, sealed requests, sign-in |
| **calimero-app-engine-wasmtime** | Wasmtime engine for `calimero:app` components |
| **calimero-sdk** | The Rust SDK apps are written with, built into `calimero:app` components by `calimero-app-build` |

The full list is in [`crates/AGENTS.md`](crates/AGENTS.md).

## Quick start

```bash
cargo run -p merod -- local                     # 3 nodes on 127.0.0.1:2501-2503; keeps running, go on in another terminal
cargo install --path crates/meroctl
meroctl node add local http://127.0.0.1:2501/ --allow-mock
meroctl login --dev-passkey ./dev-passkey.json --name ada   # signs up: an account and its personal organization
scripts/install-mero-app-build.sh                           # once: puts mero-app-build on PATH
mero-app-build apps/kv-store --out kv_store.wasm
meroctl context create --path kv_store.wasm --name kv --alias kv
meroctl call --context kv set --args '{"key":"a","value":"1"}'
meroctl call --context kv get --args '{"key":"a"}' --view
```

The full walk-through, with the JavaScript and Python clients, is the
[Quickstart](https://calimero-network.github.io/core/cloud/quickstart/).

Nodes run on Linux; on macOS, run `merod` in Docker and `meroctl` natively.
See [Install & first run](https://calimero-network.github.io/core/operate/install/).

## Documentation

| I want to… | Start here |
|-------|------|
| Understand Calimero and try it | [Cloud](https://calimero-network.github.io/core/cloud/overview/) — overview, quickstart, writing apps |
| Write a Calimero app | [Build](https://calimero-network.github.io/core/build/) — SDK, collections, examples |
| Run a cluster | [Operate](https://calimero-network.github.io/core/operate/) — install, deployment, `merod`/`meroctl`, configuration, the node API |
| Understand the model | [Protocol Reference](https://calimero-network.github.io/core/protocol/overview/) |
| Work on Calimero Core itself | [Contribute](https://calimero-network.github.io/core/contribute/) — architecture, crate guide, development, testing |

The docs live in [`docs/`](docs/) (Astro Starlight).

## Related Repositories

| Repo | What it is |
| --- | --- |
| [mero-tee](https://github.com/calimero-network/mero-tee) | TDX node images, mero-kms, signed releases |
| [app-registry](https://github.com/calimero-network/app-registry) | Where signed apps are published and installed from |
| [mdma](https://github.com/calimero-network/mdma) | Calimero Cloud's cluster manager (VMs, placement, rollouts) |
| [mero-js](https://github.com/calimero-network/mero-js) · [mero-react](https://github.com/calimero-network/mero-react) | JavaScript/TypeScript client and React bindings |
| [calimero-client-py](https://github.com/calimero-network/calimero-client-py) | Python client |
| [swift-sdk](https://github.com/calimero-network/swift-sdk) · [kotlin-sdk](https://github.com/calimero-network/kotlin-sdk) | iOS/macOS and Android clients |
| [apps](https://github.com/calimero-network/apps) | 27 apps built on Calimero |

Apps build with `calimero-app-build` (`mero-app-build`) and are signed with [`tools/mero-sign`](tools/mero-sign).

## License

MIT OR Apache-2.0
