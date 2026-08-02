# cargo-mero

`cargo mero` is the toolchain for Calimero WASM applications.
It scaffolds an app, compiles it to `wasm32-unknown-unknown` with the ABI embedded, runs the node-free test suite, and packages a signed `.mpk` bundle ready for `meroctl app install`.
One tool covers the whole path from `cargo mero new` to an installable bundle, replacing the hand-written `build.sh` / `build-bundle.sh` scripts each app used to carry.

## Install

`cargo mero` is a Cargo subcommand: install the `cargo-mero` binary and Cargo picks it up as `cargo mero`.

Install from the repository (works today):

```bash
cargo install --git https://github.com/calimero-network/core cargo-mero
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
Scaffolds a crate: `Cargo.toml` (SDK pins, the `[package.metadata.calimero]` app id, and the `app-release` / `app-profiling` profiles), `src/lib.rs` (state, events, logic, and a `#[cfg(test)]` TestHost test), and `tests/converge.rs`. No build script: the ABI is emitted by `build` below.

**2. `cargo mero build`**
Takes the ABI manifest the app itself builds, compiles to `wasm32-unknown-unknown`, copies the wasm into `res/`, size-optimizes it with `wasm-opt -Oz` (release only), and embeds the (canonicalized) full ABI as the wasm `calimero_abi_v1` custom section.
The manifest has one producer: the `__calimero_abi()` that `#[app::logic]` generates from the `AbiType` impls the app's types carry, so the compiler resolves aliases, macro-generated and re-exported types before anything is described.
Artifacts: `res/<name>.wasm` (the built, ABI-embedded wasm) plus `res/abi.json` and `res/state-schema.json`. An app needs no `build.rs` for any of this.
An app whose SDK predates `__calimero_abi` builds no manifest of its own; the build then warns, writes no `res/abi.json` or `res/state-schema.json`, and embeds no section. The wasm is still produced, but `bundle` refuses it - every bundle entry names an `abi.json`.

**3. `cargo mero test`**
Runs the native test suite - the in-crate TestHost unit tests plus the `tests/converge.rs` convergence test.
No node or network is needed.

**4. `cargo mero bundle`**
Builds every service, stages the wasm/abi files under `res/bundle-temp/`, writes `manifest.json`, signs it, and packages everything into a tar.gz `.mpk`.
Artifact: `dist/<package>.mpk` (the `<package>` is the `[package.metadata.calimero] package` id; the app version lives inside the manifest, not the filename).
Pass a signing method: `--dev` (local) or `--key <file>` (production).
See [SIGNING.md](SIGNING.md).

**5. `meroctl app install --path dist/<package>.mpk`**
Installs the bundle on a running `merod` node.
The node derives the `ApplicationId` from the bundle's `package` and signer (see [SIGNING.md](SIGNING.md)).

## Cargo features

`build` and `bundle` take cargo's feature flags, spelled the way cargo spells them - comma or space separated, repeatable:

```bash
cargo mero build --features schema_v2
cargo mero bundle --dev --features "schema_v2 telemetry" --no-default-features
```

They reach `cargo build` and the `cargo metadata` call the ABI is emitted against, so the embedded `calimero_abi_v1` section always describes the schema the bytecode was compiled with.
That pairing is the point: a wasm and an ABI that disagree do not fail the build, they produce a wrong migration plan at upgrade time, because the node resolves upgrades from the embedded section alone.

Features gate **top-level items**, which is how one crate carries two schema versions:

```rust
#[cfg(not(feature = "schema_v2"))]
#[app::state(version = 1)]
pub struct State { /* ... */ }

#[cfg(feature = "schema_v2")]
#[app::state(version = 2)]
pub struct StateV2 { /* ... */ }
```

A `#[cfg]` on a method inside an `#[app::logic]` block does not reach the ABI, which then describes a method the wasm may not export; express a variant as whole gated items rather than gated members.

ABI extraction compiles on the host, so an ABI-visible item gated on `target_arch` describes its host form rather than the wasm one; gate on features (which extraction shares with the wasm build) instead of the target.

In a multi-service workspace all services compile in one `cargo build`, so a feature only one service declares is fine: it applies to that service and is ignored by the rest.

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

A built app carries the same *full* manifest in two places.

The wasm's embedded `calimero_abi_v1` custom section holds it as compact JSON - every method (with its per-method metadata such as `xcall_callable`) and event, plus the state schema (`state_root` and its transitive types).
The node's xcall entry-point gate reads the per-method flags from this embedded section, and its migration / identity-downgrade gate reads the same section's state fields (it tolerates the extra methods/events), so `cargo mero abi diff` also compares it between versions.

The bundle's `abi.json` sidecar is the same manifest, pretty-printed.

`cargo mero build` name-sorts `methods` and `events` before embedding, which the node's `validate_manifest` requires (it silently discards a section that fails validation).
An app built against today's SDK already carries them sorted, so the sort only bites for one built against an older one.

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

The scaffolded SDK version has a single source of truth; a bump moves it and the test that pins the expected value:

- `DEFAULT_SDK_VERSION` in `tools/cargo-mero/src/main.rs` - the one value `cargo mero new` substitutes into the scaffolded `Cargo.toml` (the SDK git tag) and that `cargo mero test`'s example dev-dep hint prints.
- the version assertion in `tools/cargo-mero/src/new.rs` tests, which hardcodes the expected tag and so must be bumped in lockstep.
- `new_build_test_bundle_ladder` in `tools/cargo-mero/tests/pipeline.rs`: it asserts the degraded no-ABI path, because every tag released so far predates `__calimero_abi`. Bumping to a tag that carries it turns those last assertions back into "bundle succeeds and writes the `.mpk`".

Scaffolded apps carry no build script, so they no longer pin `calimero-wasm-abi`; the ABI comes from whichever `cargo mero` builds them.

The in-repo test fixtures use `path` dependencies on core's own SDK crates, so they need no SDK-version update.

## License

Licensed under either of Apache License, Version 2.0 or MIT license, at your option - see the repository root.
Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
