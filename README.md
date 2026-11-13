# Calimero Core

A confidential cloud for collaborative apps. Apps are WASM, written with the
Calimero SDK; each app instance is replicated by its own Raft log across
attested Intel TDX nodes, reached through a sealed channel the client opens only
after verifying the node's attestation, and signed into with passkeys. Nobody
operating the nodes can read the apps' data.

> **Full documentation**: <https://calimero-network.github.io/core/>, organized into **Cloud** (start here), **Build** (write apps), **Operate** (run a cluster), **Protocol Reference** and **Contribute** (work on core). Coming from the earlier peer-to-peer release? The [glossary](https://calimero-network.github.io/core/protocol/glossary/) maps its names to these.

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

On macOS, install `meroctl` alone and run the node in Docker
(`meroctl local up --docker`). From a checkout, `cargo install --locked --path
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
meroctl local up                                            # 3 nodes on 127.0.0.1:2501-2503 in the background, saved as node local
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
| Run locally | `local up\|down\|status [--docker]`, `local dev`, `local test`, `local compute run` |
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

- [calimero-network/mero-tee](https://github.com/calimero-network/mero-tee) -- TEE images, mero-kms, signed releases
- [calimero-network/mdma](https://github.com/calimero-network/mdma) -- Calimero's cluster manager (VMs, placement, the rebalancer)
- [calimero-network/apps](https://github.com/calimero-network/apps) -- the apps, and the templates `meroctl app new` starts from
- [calimero-network/mero-js](https://github.com/calimero-network/mero-js) -- JavaScript/TypeScript client
- [calimero-network/calimero-client-py](https://github.com/calimero-network/calimero-client-py) -- Python client
- [calimero-network/swift-sdk](https://github.com/calimero-network/swift-sdk) and [calimero-network/kotlin-sdk](https://github.com/calimero-network/kotlin-sdk) -- iOS/macOS and Android clients

## License

MIT OR Apache-2.0
