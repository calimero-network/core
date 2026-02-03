# apps/ - Example WASM Applications

Example and test WebAssembly applications demonstrating Calimero SDK usage.

## Package Identity

- **Target**: `wasm32-unknown-unknown`
- **SDK**: `calimero-sdk`
- **Storage**: `calimero-storage` (CRDTs)

## Commands

```bash
# Add WASM target (one-time)
rustup target add wasm32-unknown-unknown

# Build specific app
cargo build -p kv-store --target wasm32-unknown-unknown --release

# Build all apps
./scripts/build-all-apps.sh

# Run app's build script
cd apps/kv-store && ./build.sh
```

## Available Apps

| App                      | Purpose                | Good Example For       |
| ------------------------ | ---------------------- | ---------------------- |
| `kv-store`               | Simple key-value store | Basic CRDT usage       |
| `kv-store-init`          | KV with custom init    | `#[app::init]` pattern |
| `kv-store-with-handlers` | KV with event handlers | Event handling         |
| `access-control`         | Permission management  | Authorization patterns |
| `blobs`                  | Blob storage demo      | Blob operations        |
| `collaborative-editor`   | Collaborative text     | Complex CRDTs          |
| `team-metrics-macro`     | Metrics with macros    | Macro usage            |
| `team-metrics-custom`    | Metrics custom impl    | Custom CRDT usage      |
| `xcall-example`          | Cross-context calls    | XCall pattern          |

## App Structure

Each app follows this structure:

```
app-name/
├── Cargo.toml                # Crate config
├── build.sh                  # Build script
├── build.rs                  # Build-time config (optional)
├── README.md                 # App documentation
├── src/
│   └── lib.rs                # App implementation
└── workflows/
    └── *.yml                 # Test workflows (merobox)
```

## Patterns

### Basic App Pattern

- ✅ DO: Follow `kv-store/src/lib.rs` as reference

```rust
// apps/kv-store/src/lib.rs
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::app;
use calimero_storage::collections::UnorderedMap;

#[app::state]
#[derive(Default, BorshSerialize, BorshDeserialize)]
pub struct KvStore {
    items: UnorderedMap<String, String>,
}

#[app::logic]
impl KvStore {
    #[app::init]
    pub fn init() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: String, value: String) {
        self.items.insert(key, value);
    }

    pub fn get(&self, key: String) -> Option<String> {
        self.items.get(&key).cloned()
    }
}
```

### Cargo.toml Pattern

```toml
[package]
name = "kv-store"
version = "0.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
calimero-sdk = { workspace = true }
calimero-storage = { workspace = true }

[profile.release]
codegen-units = 1
opt-level = "z"
lto = true
debug = false
panic = "abort"
overflow-checks = true
```

### Build Script Pattern

```bash
#!/bin/bash
# build.sh
set -e
cd "$(dirname "$0")"
cargo build --target wasm32-unknown-unknown --release
```

## Key Reference Files

| File                                | Purpose         |
| ----------------------------------- | --------------- |
| `kv-store/src/lib.rs`               | Basic CRDT app  |
| `access-control/src/lib.rs`         | Auth patterns   |
| `kv-store-with-handlers/src/lib.rs` | Event handlers  |
| `blobs/src/lib.rs`                  | Blob operations |

## JIT Index

```bash
# Find all app entry points
rg -n "#\[app::state\]" */src/

# Find all public methods
rg -n "pub fn" */src/lib.rs

# Find CRDT usage
rg -n "UnorderedMap|Counter|LwwRegister|Vector" */src/

# Find event handlers
rg -n "#\[app::on_" */src/
```

## Workflows

Each app has test workflows in `workflows/` directory:

```yaml
# workflows/simple-store.yml
name: Simple KV Store Test
steps:
  - action: create_context
    app: kv-store
  - action: call
    method: set
    args: '{"key": "test", "value": "hello"}'
  - action: call
    method: get
    args: '{"key": "test"}'
    expect: '"hello"'
```

## Building for Production

```bash
# Use release profile optimized for WASM
cargo build -p <app-name> \
    --target wasm32-unknown-unknown \
    --profile app-release

# Output: target/wasm32-unknown-unknown/app-release/<app_name>.wasm
```

## Common Gotchas

- Must use `crate-type = ["cdylib"]` in Cargo.toml
- All state fields must be serializable (borsh)
- Use `--release` or `--profile app-release` for deployment
- Panic behavior in WASM differs from native
- Test locally with `meroctl call` before deployment
