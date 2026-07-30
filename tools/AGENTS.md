# tools/ - Development Tools

Development and debugging tools for Calimero infrastructure.

## Available Tools

| Tool           | Binary     | Purpose                                      |
| -------------- | ---------- | -------------------------------------------- |
| `cargo-mero`   | `cargo-mero` | App toolchain: scaffold, build, test, and bundle a signed `.mpk` |
| `merodb`       | `merodb`    | RocksDB debugging, inspection, and migration |
| `calimero-abi` | `mero-abi`  | ABI extraction and inspection from WASM      |
| `mero-sign`    | `mero-sign` | Sign Calimero bundle manifests (Ed25519)     |

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
| `new <name>` | Scaffold an app (Cargo.toml, lib.rs with a TestHost test) |
| `build` | Emit the ABI from `src/*.rs`, compile to wasm32, `wasm-opt -Oz`, embed the ABI as `calimero_abi_v1` |
| `test` | Run node-free TestHost unit tests + `tests/converge.rs` |
| `bundle` | Build all services, write + sign `manifest.json`, tar the `.mpk` |
| `abi` | Passthrough to `mero-abi`: `extract` / `state` / `types` / `inspect` / `embed` / `diff` |
| `key` | Signing-key utilities: `generate` / `derive-signer-id` (backed by `mero-sign`) |
| `sign` | Sign a bundle `manifest.json` in place |
| `guide` | Print the 5-step workflow guide (the canonical wording, in `src/guide.rs`) |

### Usage

```bash
# The five-step ladder
cargo mero new my-app
cargo mero build             # -> res/my_app.wasm (ABI embedded)
cargo mero test
cargo mero bundle --dev      # -> dist/<package>.mpk (dev key; --key <file> for prod)

# CI signing without a checked-in key
export MERO_SIGN_KEY="$RUNNER_TEMP/mero-key.json"
cargo mero bundle            # reads MERO_SIGN_KEY when no --key/--dev
```

### File Organization

```
cargo-mero/
├── Cargo.toml
├── README.md                # user-facing workflow + metadata reference
├── SIGNING.md               # dev vs prod keys, MERO_SIGN_KEY, ApplicationId derivation
├── src/
│   ├── main.rs              # CLI (DEFAULT_SDK_VERSION lives here)
│   ├── new.rs               # `new` scaffold (+ version-pin tests)
│   ├── build.rs             # `build`: emit ABI -> compile -> wasm-opt -> embed ABI
│   ├── test_cmd.rs          # `test`
│   ├── bundle.rs            # `bundle`: stage, manifest, sign, package
│   ├── manifest.rs          # manifest.json shape
│   ├── meta.rs              # [package.metadata.calimero] parsing
│   └── guide.rs             # `guide` text (canonical 5-step wording)
├── templates/              # scaffold templates for `new`
└── tests/
    ├── pipeline.rs          # end-to-end ladder tests (all #[ignore]d)
    └── fixtures/            # demo-app + multi-app crates
```

### Common Gotchas

- The `tests/pipeline.rs` end-to-end tests are all `#[ignore]`d because they scaffold and compile fresh crates (slow, and `new_build_test_bundle_ladder` needs network for the git SDK deps). Run them with `cargo test -p cargo-mero -- --ignored`.
- Bumping the scaffolded SDK version touches two files in lockstep: `DEFAULT_SDK_VERSION` in `src/main.rs` and the version assertions in `src/new.rs` tests.
- `services` is a workspace-only key: it is rejected under a `[package.metadata.calimero]` table, only accepted under `[workspace.metadata.calimero]`.
- `--features` is resolved once and used for both the compile and the ABI emit (`workspace::FeatureArgs` feeds `cargo build` and `cargo metadata`). Keep them together: a wasm and an embedded ABI that disagree fail silently, as a wrong migration plan.
- The ABI emitter cfg-filters top-level items only. A `#[cfg]` on a struct field, enum variant, or `#[app::logic]` method still lands in the ABI.

## merodb - Database Tool

### Commands

```bash
# Build
cargo build -p merodb

# With GUI feature
cargo build -p merodb --features gui

# Run
cargo run -p merodb -- [options]
```

### Features

| Feature           | Purpose                             |
| ----------------- | ----------------------------------- |
| Schema inspection | View RocksDB column families        |
| Data export       | Export database to JSON             |
| DAG visualization | Export DAG structure                |
| Validation        | Check database integrity            |
| Migration         | Database migrations with YAML specs |
| GUI               | Interactive browser-based interface |

### Usage

```bash
# View schema
cargo run -p merodb -- --db-path ~/.calimero/node1/data --schema

# Export all data
cargo run -p merodb -- --db-path ~/.calimero/node1/data \
    --export --all \
    --wasm-file ./target/wasm32-unknown-unknown/release/my_app.wasm \
    --output export.json

# Validate database
cargo run -p merodb -- --db-path ~/.calimero/node1/data --validate

# Export DAG
cargo run -p merodb -- --db-path ~/.calimero/node1/data \
    --export-dag --output dag.json

# Launch GUI
cargo run -p merodb --features gui -- --gui
```

### File Organization

```
merodb/
├── Cargo.toml
├── README.md
├── MIGRATIONS.md             # Migration documentation
├── src/
│   ├── main.rs               # CLI entry point
│   ├── schema.rs             # Schema inspection
│   ├── export.rs             # Data export
│   ├── export/
│   │   └── cli.rs            # Export CLI
│   ├── dag.rs                # DAG operations
│   ├── dag/
│   │   └── cli.rs            # DAG CLI
│   ├── validation.rs         # Validation logic
│   ├── validation/
│   │   └── cli.rs            # Validation CLI
│   ├── migration/            # Migration system
│   │   ├── cli.rs            # Migration CLI
│   │   ├── loader.rs         # YAML loader
│   │   ├── execute.rs        # Migration execution
│   │   └── ...
│   ├── gui/                  # Browser GUI
│   │   ├── mod.rs
│   │   ├── server.rs         # HTTP server
│   │   ├── index.html        # Main page
│   │   └── static/           # JS/CSS assets
│   ├── deserializer.rs       # Data deserializers
│   ├── types.rs              # Types
│   └── abi.rs                # ABI utilities
└── examples/
    ├── 01-basic-copy.yaml    # Migration examples
    └── ...
```

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
# Find merodb commands
rg -n "#\[derive.*Parser\]" merodb/src/

# Find export formats
rg -n "pub fn export" merodb/src/

# Find ABI extraction logic
rg -n "pub fn " calimero-abi/src/

# Find GUI endpoints
rg -n "\.route\(" merodb/src/gui/
```

## Common Debugging Workflows

### Inspect Database After Test

```bash
# After running tests, inspect state
cargo run -p merodb -- --db-path ~/.calimero/test-node/data --schema
cargo run -p merodb -- --db-path ~/.calimero/test-node/data --export --all
```

### Debug WASM ABI

```bash
# Check if ABI is correctly generated
cargo run -p mero-abi -- extract ./target/wasm32-unknown-unknown/release/kv_store.wasm
```

### Visualize DAG

```bash
# Export DAG for debugging sync issues
cargo run -p merodb -- --db-path ~/.calimero/node1/data \
    --export-dag --output dag.json
# Open dag.json to trace delta parent relationships
```

## Common Gotchas

- merodb requires `--db-path` for most operations
- GUI feature requires `--features gui` at build time
- ABI extraction works on compiled WASM, not source
- Database must not be in use by running node
