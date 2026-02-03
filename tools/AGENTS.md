# tools/ - Development Tools

Development and debugging tools for Calimero infrastructure.

## Available Tools

| Tool           | Binary     | Purpose                                      |
| -------------- | ---------- | -------------------------------------------- |
| `merodb`       | `merodb`   | RocksDB debugging, inspection, and migration |
| `calimero-abi` | `mero-abi` | ABI extraction and inspection from WASM      |

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

## calimero-abi - ABI Tool

### Commands

```bash
# Build
cargo build -p calimero-abi

# Run
cargo run -p calimero-abi -- [command]
```

### Features

| Command   | Purpose                    |
| --------- | -------------------------- |
| `extract` | Extract ABI from WASM file |
| `state`   | Inspect state schema       |

### Usage

```bash
# Extract ABI
cargo run -p calimero-abi -- extract ./my_app.wasm

# Inspect state schema
cargo run -p calimero-abi -- state ./my_app.wasm
```

### File Organization

```
calimero-abi/
├── Cargo.toml
└── src/
    ├── main.rs               # CLI entry point
    ├── extract.rs            # ABI extraction
    └── inspect.rs            # State inspection
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
cargo run -p calimero-abi -- extract ./target/wasm32-unknown-unknown/release/kv_store.wasm
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
