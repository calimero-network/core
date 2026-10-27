# Crates Directory - AI Agent Guidance

The crates of the Calimero cloud (release-v2). Each crate is conceptually separate;
its own `AGENTS.md` (with a `CLAUDE.md` symlink beside it) explains it.

## Binaries

| Crate | Binary | Entry point | Purpose |
| --- | --- | --- | --- |
| `calimero-cloud-node` | `merod-v2` | `cloud-node/src/main.rs` | The cloud node; `merod-v2 local` runs a dev cluster, `merod-v2 kms disk-key` fetches a TEE disk key |
| `calimero-cloud-client` | `calimero-cloud` | `cloud-client/src/bin/` | Client library and CLI |

## The cloud node and its parts

| Crate | Purpose |
| --- | --- |
| [`calimero-cloud-node`](cloud-node/AGENTS.md) | System group, app groups, sealed API, follow-ups, timers, search, blobs |
| [`calimero-log`](log/AGENTS.md) | Multi-group Raft log (openraft) |
| [`calimero-identity`](identity/AGENTS.md) | Passkeys (WebAuthn), sessions, API keys, step-up consent |
| [`calimero-sealed`](sealed/AGENTS.md) | Sealed HTTP transport to an attested node |
| [`calimero-kms-client`](kms-client/AGENTS.md) | mero-kms client: release-policy-verified storage and disk keys |
| [`calimero-tee-attestation`](tee-attestation/AGENTS.md) | TDX quote generation and verification |
| [`calimero-tee-release`](tee-release/AGENTS.md) | Fetch and Sigstore-verify signed mero-tee release assets |
| [`calimero-cloud-client`](cloud-client/AGENTS.md) | Rust client: attestation, sealed channel, passkeys, calls |

## Apps: runtime and SDK

| Crate | Purpose |
| --- | --- |
| [`calimero-runtime`](runtime/AGENTS.md) | WASM execution (wasmer); merod-v2 runs it with `Module::run_cloud` |
| [`calimero-sdk`](sdk/AGENTS.md) | App SDK and proc macros |
| [`calimero-storage`](storage/AGENTS.md) | CRDT collections and the storage interface |
| [`calimero-storage-macros`](storage-macros/AGENTS.md) | Storage derive macros |
| [`calimero-sys`](sys/AGENTS.md) | WASM host ABI types |
| [`calimero-wasm-abi`](wasm-abi/AGENTS.md) | ABI schema: emit, validate, embed |
| [`calimero-search`](search/AGENTS.md) | Full-text search (tantivy) |
| [`calimero-prelude`](prelude/AGENTS.md) | Shared root-storage-key prelude |
| [`calimero-bundle`](bundle/AGENTS.md) | `.mpk` manifest types, used by `cargo-mero` and `mero-sign` |

## Shared types

| Crate | Purpose |
| --- | --- |
| [`calimero-primitives`](primitives/AGENTS.md) | Shared types: `ContextId`, `ApplicationId`, `PublicKey`, `Hash` |
| [`calimero-crypto`](crypto/AGENTS.md) | Sealing to a root key (`seal_to_root`) |
| [`calimero-build-utils`](build-utils/AGENTS.md) | build.rs version/git helpers |
| [`calimero-git-hooks`](git-hooks/AGENTS.md) | Self-installing pre-commit hook |
| `calimero-utils-fs` | File helpers for `mero-sign` |

## Left from v1, still compiled

v1's node, server, network, context and governance crates are gone. These remain
because the runtime (its `NodeClient` blob path and `GovernanceParentEdge`) or
`calimero-tee-attestation` (`server-primitives`' `Quote`) still name their types;
taking the runtime's v1 path out lets them go too.

`calimero-node-primitives`, `calimero-context-client` (`context/primitives`),
`calimero-context-config`, `calimero-network-primitives`, `calimero-server-primitives`,
`calimero-store`, `calimero-blobstore` (`store/blobs`), `calimero-app-downloader`,
`calimero-governance-types`, `calimero-account`, `calimero-utils-actix`.

## Commands

```bash
cargo test -p calimero-cloud-node     # 3-node cluster in process (fixtures: cloud-node/AGENTS.md)
cargo run -p calimero-cloud-node --bin merod-v2 -- local
cargo clippy --workspace --all-targets --features calimero-storage/testing -- -D warnings
```

Benchmarks and cost gates: see `docs/benchmarking.md`. Cost gates block; criterion never does.
