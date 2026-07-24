# cargo-mero

`cargo mero` is the toolchain for Calimero WASM applications.
It scaffolds an app, compiles it to `wasm32-unknown-unknown` with the ABI embedded, runs the node-free test suite, and packages a signed `.mpk` bundle ready for `meroctl app install`.
One tool covers the whole path from `cargo mero new` to an installable bundle, replacing the hand-written `build.sh` / `build-bundle.sh` scripts each app used to carry.

## Install

`cargo mero` is a Cargo subcommand: install the `cargo-mero` binary and Cargo picks it up as `cargo mero`.

Install from the repository (works today):

```bash
cargo install --git https://github.com/calimero-network/cargo-mero cargo-mero
```

Once published, `cargo install cargo-mero` will install it from crates.io, and prebuilt binaries will be attached to the repository's releases page.

The build step also needs the `wasm32-unknown-unknown` target:

```bash
rustup target add wasm32-unknown-unknown
```

`cargo mero build` auto-installs the target when `rustup` is available.
Size-optimization with `wasm-opt -Oz` runs automatically on every release build: the optimizer is compiled into `cargo-mero` through the bundled [`wasm-opt`](https://crates.io/crates/wasm-opt) crate, so there is nothing to install on `PATH` and the optimized output is reproducible across machines.

## The workflow

The five steps below match `cargo mero guide` (the canonical source; run it any time).

```bash
cargo mero new my-app        # 1. scaffold an app (state, events, logic, tests)
cargo mero build             # 2. compile -> wasm-opt -> embed ABI (res/my_app.wasm)
cargo mero test              # 3. run TestHost unit tests + convergence tests (no node needed)
cargo mero bundle --dev      # 4. build all services, write manifest.json, sign, tar dist/<package>.mpk
meroctl app install --path dist/<package>.mpk ...   # 5. install on a node (merod)
```

**1. `cargo mero new my-app`**
Scaffolds a crate: `Cargo.toml` (SDK pins, the `[package.metadata.calimero]` app id, and the `app-release` / `app-profiling` profiles), `build.rs`, `src/lib.rs` (state, events, logic, and a `#[cfg(test)]` TestHost test), and `tests/converge.rs`.

**2. `cargo mero build`**
Compiles to `wasm32-unknown-unknown`, copies the wasm into `res/`, size-optimizes it with `wasm-opt -Oz` (release only), and embeds the (canonicalized) full ABI as the wasm `calimero_abi_v1` custom section.
Artifacts: `res/<name>.wasm` (the built, ABI-embedded wasm) plus `res/abi.json` and `res/state-schema.json` emitted by the app's `build.rs`.

**3. `cargo mero test`**
Runs the native test suite - the in-crate TestHost unit tests plus the `tests/converge.rs` convergence test.
No node or network is needed.

**4. `cargo mero bundle`**
Builds every service, stages the wasm/abi files under `res/bundle-temp/`, writes `manifest.json`, signs it, and packages everything into a tar.gz `.mpk`.
Artifact: `dist/<package>.mpk` (the `<package>` is the `[package.metadata.calimero] package` id; the app version lives inside the manifest, not the filename).
Pass a signing method: `--dev` (local), `--key <file>` (production), or `--unsigned`.
See [SIGNING.md](SIGNING.md).

**5. `meroctl app install --path dist/<package>.mpk`**
Installs the bundle on a running `merod` node.
The node derives the `ApplicationId` from the bundle's `package` and signer (see [SIGNING.md](SIGNING.md)).

## Metadata reference

Bundle metadata comes from a `[package.metadata.calimero]` table in the app's `Cargo.toml` (or `[workspace.metadata.calimero]` for a multi-service workspace).
Keys are kebab-case.
The workspace table wins over the package table when both are present.

| `Cargo.toml` key      | `manifest.json` field  | Default                          |
| --------------------- | ---------------------- | -------------------------------- |
| `package`             | `package`              | required (no default)            |
| `name`                | `metadata.name`        | the crate name                   |
| `description`         | `metadata.description` | omitted                          |
| `author`              | `metadata.author`      | omitted                          |
| `min-runtime-version` | `minRuntimeVersion`    | `0.1.0`                          |
| `frontend`            | `links.frontend`       | omitted                          |
| `services`            | `services[]`           | empty (workspace table only)     |

The app version (`manifest.json` `appVersion`) is not a metadata key.
It defaults to the crate's `[package] version` and is overridable with `cargo mero bundle --app-version <v>`.
The `manifest.json` `version` field is the manifest schema version and is always `1.0`.

Single-service `Cargo.toml`:

```toml
[package.metadata.calimero]
package = "com.example.my-app"
name = "My App"
description = "A collaborative example app"
min-runtime-version = "0.7.0"
frontend = "https://my-app.example.com"
```

### Multi-service workspaces

A workspace that ships several wasm services declares them under `[workspace.metadata.calimero]`.
Each `services` entry maps a bundle service `name` to the `crate` that builds it; `services` under a `[package.metadata.calimero]` table is rejected.
The bundle then emits `services/<name>.wasm` + `services/<name>-abi.json` per service instead of a top-level `app.wasm` / `abi.json`.

```toml
# workspace root Cargo.toml
[workspace.metadata.calimero]
package = "network.calimero.mero-drive"
name = "Mero Drive"
description = "Collaborative file storage"
min-runtime-version = "0.7.0"
frontend = "https://drive.calimero.network"

[[workspace.metadata.calimero.services]]
name = "drive"
crate = "mero-drive-service"

[[workspace.metadata.calimero.services]]
name = "index"
crate = "mero-index-service"
```

## Two ABI payloads

A built app carries the ABI in two distinct places. They hold the same *full* manifest but differ in ordering.

The wasm's embedded `calimero_abi_v1` custom section holds the **canonicalized full ABI** - every method (with its per-method metadata such as `xcall_callable`) and event, plus the state schema (`state_root` and its transitive types).
The node's xcall entry-point gate reads the per-method flags from this embedded section, and its migration / identity-downgrade gate reads the same section's state fields (it tolerates the extra methods/events), so `cargo mero abi diff` also compares it between versions.

The bundle's `abi.json` sidecar is the same full ABI **as emitted** by the SDK, i.e. methods and events in source-declaration order.

"Canonicalized" means the embedded copy has its `methods` and `events` arrays sorted by name.
The SDK emitter writes them in source order, but the node's `validate_manifest` requires them name-sorted and silently discards a section that fails validation, so `cargo mero build` sorts the arrays before embedding.
This sort is a workaround for that emitter/validator ordering mismatch (a core bug being filed upstream); once core accepts source-order manifests it can be dropped and the two payloads become byte-identical.

## Links

- Calimero core documentation: <https://calimero-network.github.io/core/>
- merobox (networked end-to-end testing with real nodes): <https://github.com/calimero-network/merobox>
- Signing guide: [SIGNING.md](SIGNING.md)

## Repository layout

- `tools/cargo-mero` - the `cargo mero` CLI (scaffold, build, test, bundle, plus abi/key/sign passthroughs)
- `tools/calimero-abi` (crate `mero-abi`) - extracts and embeds the WASM ABI (backs `cargo mero abi`)
- `tools/mero-sign` - Ed25519 signing, key generation, and did:key derivation (backs `cargo mero key` / `cargo mero sign`)

## Design notes

A `cargo-mero` tool previously lived in `calimero-network/core` (`tools/cargo-mero`, added in core#1317, moved to `tools/` in core#1512) offering `cargo mero new` and `cargo mero build` - app scaffolding plus a thin `cargo build --target wasm32-unknown-unknown` wrapper.
It was removed in core#1518 with no documented rationale (template-only PR body, terse commit message); the removal landed the same day as unrelated tools-versioning/publishing cleanup in that directory, which is the likeliest driver rather than a flaw in the concept itself.
The tool now lives back in core's workspace at `tools/cargo-mero`, versioned and released alongside the rest of core, so that prior friction does not recur.

## Bumping the SDK version

The scaffolded SDK version is pinned in several places that must move together.
When bumping to a new `calimero-sdk` / `calimero-wasm-abi` release, update all of:

- `DEFAULT_SDK_VERSION` in `tools/cargo-mero/src/main.rs` (the single source used for `cargo mero new`'s default and `cargo mero test`'s example dev-dep hint).
- the `calimero-wasm-abi` pin in the workspace `Cargo.toml` (`[workspace.dependencies]`).
- the SDK tags in the test fixtures: `tools/cargo-mero/tests/fixtures/demo-app/Cargo.toml` and `tools/cargo-mero/tests/fixtures/multi-app/crates/*/Cargo.toml`.
- the version assertions in `tools/cargo-mero/src/new.rs` tests.

## License

Licensed under either of Apache License, Version 2.0 or MIT license, at your option - see the repository root.
Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
