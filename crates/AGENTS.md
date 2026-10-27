# Crates - AI Agent Guidance

The workspace's library and binary crates on release-v2. Each has its own
`AGENTS.md` (with a `CLAUDE.md` symlink beside it) where one exists; read it
before working in the crate. How they connect:
[architecture](../docs/src/content/docs/contribute/architecture.mdx); the same
list with more detail: [crate guide](../docs/src/content/docs/contribute/crate-guide.mdx).

## Binaries

- [merod](merod/AGENTS.md) (`merod`) - the node: `init`, `run`, `config`, `local`, `inspect`, `kms disk-key`; turns `NodeArgs` into `calimero_cloud_node::Config`.
- [meroctl](meroctl/AGENTS.md) (`meroctl`) - the command line for nodes, v1's command tree over the attested, sealed client.

## The node

- [cloud-node](cloud-node/AGENTS.md) (`calimero-cloud-node`) - the node itself: system group, app groups, routing, the HTTP and Ops API, packages, follow-ups, blobs, presence, search, app frontends, backups, storage key. Its `AGENTS.md` is the node's mental model.
- [log](log/AGENTS.md) (`calimero-log`) - multi-group Raft (openraft) over one sealed RocksDB per node; snapshots; the `log.redb` migration.
- [sealed](sealed/AGENTS.md) (`calimero-sealed`) - the sealed HTTP transport (wire format v2), server middleware and client; byte-compatible with mero-js.
- [identity](identity/AGENTS.md) (`calimero-identity`) - WebAuthn passkeys, session tokens, API keys, confirm challenges.
- [cloud-client](cloud-client/AGENTS.md) (`calimero-cloud-client`) - the Rust client; `attest::verify` is the one quote check clients and peers share.
- [kms-client](kms-client/AGENTS.md) (`calimero-kms-client`) - the mero-kms client: release policy, key release, `kms disk-key`.
- [outbound](outbound/AGENTS.md) (`calimero-outbound`) - the gated outbound HTTPS client and secrets for runs.
- [search](search/AGENTS.md) (`calimero-search`) - per-app full-text search (tantivy), derived in memory.

## Attestation and releases

- [tee-attestation](tee-attestation/AGENTS.md) (`calimero-tee-attestation`) - TDX quote generation and DCAP verification; `mock-attestation` for tests.
- [tee-release](tee-release/AGENTS.md) (`calimero-tee-release`) - fetch and Sigstore-verify signed mero-tee release assets.

## Packages

- [bundle](bundle/AGENTS.md) (`calimero-bundle`) - the `.mpk` manifest, JCS canonicalization, Ed25519 signatures.
- [app-downloader](app-downloader/AGENTS.md) (`calimero-app-downloader`) - fetching signed releases from a package registry.

## SDK and runtime (unchanged from v1; the runtime is being replaced)

- [sdk](sdk/AGENTS.md) (`calimero-sdk`, with `sdk/macros`, `calimero-sdk-macros`) - the app SDK and `TestHost`.
- [storage](storage/AGENTS.md) (`calimero-storage`) - CRDT collections and the storage interface; `converge` harness.
- [storage-macros](storage-macros/AGENTS.md) (`calimero-storage-macros`) - derive macros for storage entities.
- [sys](sys/AGENTS.md) (`calimero-sys`) - the WASM host ABI.
- [wasm-abi](wasm-abi/AGENTS.md) (`calimero-wasm-abi`) - the `wasm-abi/1` schema and the embedded section the node reads.
- [prelude](prelude/AGENTS.md) (`calimero-prelude`) - root storage key derivation shared by SDK and storage.
- [runtime](runtime/AGENTS.md) (`calimero-runtime`) - the wasmer runtime, metering and host functions.

## Shared types

- [primitives](primitives/AGENTS.md) (`calimero-primitives`) - ids, keys, hashes, wire types.
- [account](account/AGENTS.md) (`calimero-account`) - `AccountId` (and v1's device ids and credentials).

## Runtime-only v1 crates

Kept only because `calimero-runtime` still links them; nothing in the
release-v2 node uses them directly. Do not build on them.

- `node/primitives` (`calimero-node-primitives`) - the runtime's `NodeClient`; brings in the crates below.
- `context/config` (`calimero-context-config`) - types the runtime and `governance-types` name.
- `network/primitives` (`calimero-network-primitives`) - through `node-primitives`.
- [governance-types](governance-types/AGENTS.md) (`calimero-governance-types`) - through `node-primitives`.
- [store](store/AGENTS.md) (`calimero-store`, with `store/impl/rocksdb`, `calimero-store-rocksdb`) - through `node-primitives` and the runtime's tests.
- `store/blobs` (`calimero-blobstore`) - through `node-primitives`.
- [utils](utils/AGENTS.md) (`utils/actix`, `calimero-utils-actix`) - through `node-primitives`.
- [crypto](crypto/AGENTS.md) (`calimero-crypto`) - the runtime's sealing of values to keys.

## Build and development

- [build-utils](build-utils/AGENTS.md) (`calimero-build-utils`) - `build.rs` helpers: release version, git and rustc metadata.
- [git-hooks](git-hooks/AGENTS.md) (`calimero-git-hooks`) - installs `.githooks/pre-commit` on any build.
- `utils/fs` (`calimero-utils-fs`) - writing an owner-only secret file on Unix and Windows; used by `tools/mero-sign`.

Tools (`cargo-mero`, `mero-abi`, `mero-sign`, `storage-cost`, `storage-compare`,
`state-disk-cost`, `search-bench`) are under [`../tools`](../tools/AGENTS.md).

## Deleted with v1

`merod`/`meroctl` v1 internals, `auth` (`mero-auth`), `authz`, `client`,
`config`, `context` (except `context/config`), `dag`, `governance-store`,
`network` (except `primitives`), `node` (except `primitives`), `op`,
`op-adapter`, `projection`, `server` (with `server/primitives`),
`store/encryption`, `merodb`. A document that describes them
as current is stale.

## Commands

```bash
cargo build -p merod -p meroctl
cargo test -p <package>                  # e.g. calimero-cloud-node, meroctl, calimero-log
cargo clippy -p <package> --all-targets -- -D warnings
```

The node's cluster and CLI tests need compiled app fixtures; see
[Testing strategy](../docs/src/content/docs/contribute/testing.mdx#fixtures).
Benchmarks and cost gates: [`docs/benchmarking.md`](../docs/benchmarking.md).
