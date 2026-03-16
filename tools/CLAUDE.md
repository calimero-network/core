# tools/ - Development Tools

Development and debugging tools for Calimero infrastructure.

| Tool | Binary | Purpose |
|---|---|---|
| `merodb` | `merodb` | RocksDB inspection, export, DAG viz, migration |
| `calimero-abi` | `mero-abi` | ABI extraction and state inspection from WASM |

## merodb

### Build & Run

```bash
cargo build -p merodb
cargo build -p merodb --features gui    # with browser GUI
cargo run -p merodb -- [options]
```

### Common Operations

```bash
# Inspect schema
cargo run -p merodb -- --db-path ~/.calimero/node1/data --schema

# Export all data (requires WASM for decoding)
cargo run -p merodb -- --db-path ~/.calimero/node1/data \
    --export --all \
    --wasm-file ./target/wasm32-unknown-unknown/release/my_app.wasm \
    --output export.json

# Validate database integrity
cargo run -p merodb -- --db-path ~/.calimero/node1/data --validate

# Export DAG structure
cargo run -p merodb -- --db-path ~/.calimero/node1/data \
    --export-dag --output dag.json

# Launch GUI
cargo run -p merodb --features gui -- --gui
```

### File Layout

```
merodb/src/
├── main.rs           # CLI entry
├── schema.rs         # Schema inspection
├── export.rs         # Data export
├── dag.rs            # DAG operations
├── validation.rs     # Validation
├── migration/        # YAML-based migrations
│   ├── cli.rs
│   ├── loader.rs
│   └── execute.rs
└── gui/              # Browser GUI (feature-gated)
    ├── server.rs
    └── index.html
```

## mero-abi

### Build & Run

```bash
cargo build -p mero-abi
cargo run -p mero-abi -- extract ./my_app.wasm
cargo run -p mero-abi -- state   ./my_app.wasm
```

### File Layout

```
calimero-abi/src/
├── main.rs       # CLI entry
├── extract.rs    # ABI extraction
└── inspect.rs    # State inspection
```

## Debug Workflows

```bash
# After a test run — inspect state
cargo run -p merodb -- --db-path ~/.calimero/test-node/data --schema

# Verify ABI generation is correct
cargo run -p mero-abi -- extract ./target/wasm32-unknown-unknown/release/kv_store.wasm

# Trace sync issues via DAG parents
cargo run -p merodb -- --db-path ~/.calimero/node1/data \
    --export-dag --output dag.json
```

## Quick Search

```bash
rg -n "#\[derive.*Parser\]" merodb/src/
rg -n "pub fn export" merodb/src/
rg -n "pub fn " calimero-abi/src/
rg -n "\.route\(" merodb/src/gui/
```

## Gotchas

- `--db-path` required for most merodb operations
- GUI requires `--features gui` at build time
- ABI extraction works on compiled WASM, not source
- Database must not be in use by a running node when using merodb
