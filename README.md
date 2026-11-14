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
| **meroctl** | The one command line: starting, building, signing and publishing apps (`app`), a local development cluster (`local`), Calimero Cloud (`cloud`), app instances (`context`), organizations, teams, blobs, accounts, and `ops` for the operator |
| **calimero-node** | The node's inside: system group, app groups, the HTTP and Ops API |
| **calimero-log** | Multi-group Raft (openraft) over a sealed RocksDB store |
| **calimero-sealed** | The sealed HTTP transport, server and client |
| **calimero-identity** | Passkeys, sessions, API keys, confirmation challenges |
| **calimero-client** | Rust client: attestation check, sealed requests, sign-in |
| **calimero-app-engine-wasmtime** | Wasmtime engine for `calimero:app` components |
| **calimero-sdk** | The Rust SDK apps are written with, built into `calimero:app` components by `meroctl app build` (`calimero-app-build`) |

The full list is in [`crates/AGENTS.md`](crates/AGENTS.md).

## Quick start

Install `merod` (the node, Linux) and `meroctl` (the one command line) from the
latest release, each checked against its `SHA256SUMS`:

```bash
curl -fsSL https://raw.githubusercontent.com/calimero-network/core/master/scripts/install.sh | sh -s -- merod meroctl
```

Both ship for Linux (x86_64, aarch64) and macOS (Apple silicon), also through
Homebrew (`brew install calimero-network/tap/meroctl calimero-network/tap/merod`). From a checkout, `cargo install --locked --path
crates/merod` (and `crates/meroctl`) builds them instead. See
[Install & first run](https://calimero-network.github.io/core/operate/install/).

### Start an app

```bash
meroctl app new my-app && cd my-app       # the KV Store reference app, contract and frontend
pnpm install
meroctl local dev apps/kv-store           # a local cluster, build, upload, frontend on :5173, rebuild on save
```

### By hand

```bash
meroctl local up                                            # 3 nodes on 127.0.0.1:2501-2503 in Docker, saved as node local (--local: merod from PATH)
meroctl login --dev-passkey ./dev-passkey.json --name ada   # signs up: an account and its personal organization
meroctl app build apps/kv-store --out kv_store.wasm
meroctl context create --path kv_store.wasm --name kv --alias kv
meroctl call --context kv set --args '{"key":"a","value":"1"}'
meroctl call --context kv get --args '{"key":"a"}' --view
meroctl local down
```

The full walk-through, with the JavaScript and Python clients, is the
[Quickstart](https://calimero-network.github.io/core/cloud/quickstart/).

## One command line

`meroctl` takes an app from an empty directory to production:

| Stage | Commands |
|-------|----------|
| Start | `app new DIR [--template T]` |
| Build | `app build`, `app codegen --lang ts\|python\|swift\|kotlin\|openapi`, `app abi show\|diff` |
| Ship | `app signer new`, `app pack [--key K] [--previous OLD]`, `app sign`, `app inspect`, `app publish [--org ORG]` |
| Run locally | `local up\|down\|status [--local]` (Docker by default), `local dev`, `local test`, `local compute run` |
| Cloud | `cloud login`, `cloud orgs`, `cloud clusters list\|create`, `cloud use CLUSTER`, `cloud compute pipeline\|logs` |
| Use a node | `app install`, `context`, `call`, `blob`, `file`, `namespace` (organizations), `group` (teams), `account`, `attest`, `ops` |

Every command and flag: [meroctl reference](https://calimero-network.github.io/core/operate/meroctl/).

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
| [apps](https://github.com/calimero-network/apps) | The apps built on Calimero, and the templates `meroctl app new` starts from |
| [mero-js](https://github.com/calimero-network/mero-js) · [mero-react](https://github.com/calimero-network/mero-react) | JavaScript/TypeScript client and React bindings |
| [calimero-client-py](https://github.com/calimero-network/calimero-client-py) | Python client |
| [swift-sdk](https://github.com/calimero-network/swift-sdk) · [kotlin-sdk](https://github.com/calimero-network/kotlin-sdk) | iOS/macOS and Android clients |

## License

MIT OR Apache-2.0
