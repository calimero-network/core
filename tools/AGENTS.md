# tools/ - Development Tools

Development tools for Calimero apps and the storage layer. A stopped node's
store is inspected with `merod inspect` (`crates/merod`).

## Available Tools

| Tool           | Binary     | Purpose                                      |
| -------------- | ---------- | -------------------------------------------- |
| `cargo-mero`   | `cargo-mero` | App toolchain: scaffold, build, test, and bundle a signed `.mpk` |
| `calimero-abi` | `mero-abi`  | ABI extraction and inspection from WASM      |
| `mero-sign`    | `mero-sign` | Sign Calimero bundle manifests (Ed25519)     |
| `storage-cost` | `storage-cost` | Deterministic cost gate for `calimero-storage`: rows touched and SHA-256 work per workload, diffed against `storage-costs.json` (see `docs/benchmarking.md`) |
| `state-disk-cost` | `state-disk-cost` | On-disk RocksDB bytes per state entry (kv, chat) under candidate column-family options (`README.md` holds the results) |
| `search-bench` | `search-bench` | `calimero-search`'s engine over the node's `RamDirectory` (and `MmapDirectory`): index size, query latency, memory, the search-gas fit; `README.md` holds the results, CI's `benchmarks.yml` uploads a report |
| `storage-compare` | `storage-compare` | State bytes, delta bytes and per-call time for kv and chat workloads, built at any commit to compare two of them (`RESULTS.md` holds the before/after results) |

Everything here is a Rust crate.

## storage-cost - Storage Cost Gate

Runs every workload in `src/workloads.rs` against a counting in-memory store and emits, per workload and size, the state and index rows read, written and removed, plus `hash_calls` / `hash_blocks`: the SHA-256 hashes `calimero-storage` finished and the compression blocks they ran, a deterministic proxy for CPU.
`scripts/check-storage-cost.sh` diffs that against `storage-costs.json` (CI's `storage-cost` job); `docs/benchmarking.md` is the user-facing page.

```bash
./scripts/check-storage-cost.sh                                   # the gate
cargo run -p storage-cost --bin storage-cost --release \
    > tools/storage-cost/storage-costs.json                       # accept a change
cargo test -p storage-cost --release -- --include-ignored         # shape + tolerance tests
```

- Hash counts come from `calimero-storage`'s `cost-meter` feature (`crates/storage/src/hash_meter.rs`), which only this crate enables. Without it `hash_meter::Sha256` is `sha2::Sha256`; with it, a wrapper that produces the same digest and bumps a thread-local counter. Storage code must hash through `crate::hash_meter`, never `sha2` directly (`production_code_hashes_through_the_meter` refuses it), or that hashing is invisible to the gate.
- Cargo unifies features across a workspace build, so `cargo build --workspace` compiles the counter into everything it builds. Digests, stored bytes and gas are identical either way; release builds (`-p merod ...`) and app wasm builds do not include `storage-cost` and get plain `sha2`.
- `measure` charges the thread's hashing to the innermost in-flight measurement; `reset_counters` discards it along with the rows.

## cargo-mero - App Toolchain

The supported build path for Calimero apps: one Cargo subcommand from `cargo mero new` to a signed `.mpk`.
It replaces the hand-rolled `build.sh` / `build-bundle.sh` scripts.
User-facing docs live in `tools/cargo-mero/README.md`, `tools/cargo-mero/SIGNING.md`, and the docs site page `docs/src/content/docs/build/cargo-mero.mdx`.

### Commands

```bash
# Build / install
cargo build -p cargo-mero
cargo install --path tools/cargo-mero      # picked up as `cargo mero`
```

| Command | Purpose |
| ------- | ------- |
| `new <name>` | Scaffold an app (Cargo.toml, lib.rs with a TestHost test, GUIDE.md) |
| `build` | Emit the ABI from `src/*.rs`, compile to wasm32, `wasm-opt -Oz`, embed the ABI as `calimero_abi_v1` |
| `test` | Run node-free TestHost unit tests + `tests/converge.rs` |
| `bundle` | Build all services, write + sign `manifest.json`, tar the `.mpk` |
| `abi` | Passthrough to `mero-abi`: `extract` / `state` / `types` / `inspect` / `embed` / `diff` |
| `key` | Signing-key utilities: `generate` / `derive-signer-id` (backed by `mero-sign`) |
| `sign` | Sign a bundle `manifest.json` in place |
| `publish` | Publish a signed `.mpk` to the app registry |
| `guide` | Print the 5-step workflow guide (the canonical wording, in `src/guide.rs`) |

### Usage

```bash
# The five-step ladder
cargo mero new my-app
cargo mero build             # -> res/my_app.wasm (ABI embedded)
cargo mero test
cargo mero bundle --dev --no-icon      # -> dist/<package>-<appVersion>.mpk (dev key; --key <file> for prod)

# CI signing without a checked-in key
export MERO_SIGN_KEY="$RUNNER_TEMP/mero-key.json"
cargo mero bundle --no-icon  # reads MERO_SIGN_KEY when no --key/--dev

# Ship a built web frontend in the bundle, or check one in an existing .mpk
cargo mero bundle --dev --no-icon --frontend-dir web/dist --format json
cargo mero bundle --check-frontend dist/<package>-<appVersion>.mpk

# Registry: bump to the next published version and publish
cargo mero bundle --key my-key.json --bump patch
CALIMERO_API_KEY=... cargo mero publish dist/<package>-<appVersion>.mpk
```

`bundle` refuses to run without an icon decision: `icon = "<path>"` in `[package.metadata.calimero]`, or `--no-icon`.

### File Organization

```
cargo-mero/
├── Cargo.toml
├── README.md                # user-facing workflow + metadata reference
├── SIGNING.md               # dev vs prod keys, MERO_SIGN_KEY, ApplicationId derivation
├── src/
│   ├── main.rs          # fn main() { cargo_mero::run() } - all logic lives in lib.rs
│   ├── lib.rs           # CLI (clap) + dispatch; DEFAULT_SDK_VERSION lives here
│   ├── new.rs           # `new` scaffold (+ version-pin tests)
│   ├── build.rs         # `build`: emit ABI -> compile -> wasm-opt -> embed ABI
│   ├── test_cmd.rs      # `test`
│   ├── bundle.rs        # `bundle`: stage, manifest, sign, package
│   ├── manifest.rs      # constructs calimero-bundle's BundleManifest
│   ├── meta.rs          # [package.metadata.calimero] parsing
│   ├── icon.rs          # PNG -> data: URI encoding + the --no-icon/"default" policy
│   ├── logo.rs          # terminal icon preview (ANSI half-blocks)
│   ├── registry.rs      # app registry client: --bump, `publish`
│   └── guide.rs         # `guide` text (canonical 5-step wording)
├── templates/          # scaffold templates for `new`
└── tests/
    ├── pipeline.rs               # end-to-end ladder tests (mostly #[ignore]d)
    ├── manifest_roundtrip.rs     # renders through calimero-bundle's own BundleManifest type
    └── fixtures/                 # demo-app + multi-app crates
```

### Common Gotchas

- Five of the six `tests/pipeline.rs` end-to-end tests are `#[ignore]`d because they compile fresh crates (slow, and `new_build_test_bundle_ladder` needs network for the git SDK deps); run those with `cargo test -p cargo-mero -- --ignored`. `bundle_produces_signed_mpk` runs in the default `cargo test -p cargo-mero` - its fixture uses path deps, so it needs no network.
- The scaffolded SDK version (`DEFAULT_SDK_VERSION` in `src/lib.rs`) is derived at build time from `[workspace.metadata.workspaces].version` in the workspace root `Cargo.toml` (`build.rs`, via `calimero-build-utils`), or from `CARGO_PKG_VERSION` in a crates.io build, which `cargo ws publish` sets to that same version; nothing to bump by hand.
- A build of a version-bump commit made before `release.yml`'s `prepare` job tags it scaffolds against a tag that does not exist yet; it resolves once the release run creates the tag.
- `services` lives under `[workspace.metadata.calimero]`, which wins when both are present, or under `[package.metadata.calimero]` for a crate that cannot own a workspace table of its own; either way a `crate` entry may name the declaring package itself, and two entries naming the same crate build it once and stage it under both names.
- `--features` is resolved once and used for both the compile and the ABI extraction (`workspace::FeatureArgs` feeds `cargo build` and `cargo metadata`). Keep them together: a wasm and an embedded ABI that disagree fail silently, as a wrong migration plan.
- A `#[cfg]` on an `#[app::logic]` method still lands in the ABI: the attribute macro sees the method before cfg is applied. Struct fields and enum variants are cfg-stripped before the derive runs, so those are honored.
- `cargo-mero` has a lib target (`src/lib.rs`), not just a binary: `main.rs` is a one-line `cargo_mero::run()` call, and `manifest`/`meta` are `pub mod` so `tests/*.rs` can drive them directly instead of spawning a subprocess.
- `manifest::render` builds `calimero_bundle::BundleManifest` directly, with no `..Default::default()` and no `..` in the node's `BundleManifest::artifacts()` destructure - a new field on that type is a compile error here until handled, not silent drift.

## mero-abi - ABI Tool

### Commands

```bash
# Build
cargo build -p mero-abi

# Run
cargo run -p mero-abi -- [command]
```

### Features

| Command   | Purpose                    |
| --------- | -------------------------- |
| `extract` | Extract ABI from WASM file |
| `types`   | Extract only the types schema from a WASM file |
| `state`   | Extract the state schema (state root + its type dependencies) |
| `inspect` | Inspect WASM file sections |
| `embed`   | Embed a state-schema.json into a WASM as the `calimero_abi_v1` section (in place) |
| `diff`    | Diff two state-schema.json versions; flag breaking + unsafe identity downgrades |

### Usage

```bash
# Extract ABI
cargo run -p mero-abi -- extract ./my_app.wasm

# Inspect state schema
cargo run -p mero-abi -- state ./my_app.wasm
```

### File Organization

```
calimero-abi/
├── Cargo.toml
└── src/
    ├── main.rs               # CLI entry point
    ├── extract.rs            # ABI extraction
    ├── inspect.rs            # State inspection
    ├── diff.rs               # Schema diffing
    └── embed.rs              # Schema embedding
```

## JIT Index

```bash
# Find ABI extraction logic
rg -n "pub fn " calimero-abi/src/
# Find cargo-mero subcommands
rg -n "enum .*Command" cargo-mero/src/lib.rs
```

## Common Debugging Workflows

```bash
# Check that an app's ABI is embedded
cargo run -p mero-abi -- extract ./target/wasm32-unknown-unknown/app-release/kv_store.wasm
# A stopped node's store (apps, keys, log)
cargo run -p merod -- --node n1 inspect --help
```

## Common Gotchas

- ABI extraction works on compiled WASM, not source; build apps with `cargo mero build` so the ABI is embedded.
