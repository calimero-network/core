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
| `demo-blockchain-integrations` | NEAR/ICP/Ethereum proposals | Blockchain integration E2E |
| `kv-store`               | Simple key-value store | Basic CRDT usage       |
| `kv-store-init`          | KV with custom init    | `#[app::init]` pattern |
| `kv-store-with-handlers` | KV with event handlers | Event handling         |
| `migrations/migration-suite-v1..v5` | Migration examples     | Schema change patterns |
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
// Simplified example based on apps/kv-store/src/lib.rs
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct KvStore {
    items: UnorderedMap<String, LwwRegister<String>>,
}

#[app::event]
pub enum Event<'a> {
    Inserted { key: &'a str, value: &'a str },
    Updated { key: &'a str, value: &'a str },
    Removed { key: &'a str },
    Cleared,
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("key not found: {0}")]
    NotFound(&'a str),
}

#[app::logic]
impl KvStore {
    #[app::init]
    pub fn init() -> KvStore {
        KvStore {
            items: UnorderedMap::new(),
        }
    }

    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        app::log!("Setting key: {:?} to value: {:?}", key, value);

        if self.items.contains(&key)? {
            app::emit!(Event::Updated {
                key: &key,
                value: &value,
            });
        } else {
            app::emit!(Event::Inserted {
                key: &key,
                value: &value,
            });
        }

        self.items.insert(key, value.into())?;

        Ok(())
    }

    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        app::log!("Getting key: {:?}", key);

        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }
}
```

**Key points:**

- Use `#[app::state(emits = for<'a> Event<'a>)]` to declare events
- Use nested CRDTs: `UnorderedMap<String, LwwRegister<String>>` for last-write-wins semantics
- Return `app::Result<T>` from methods (not plain `T` or `Option<T>`)
- Use `app::log!` for logging and `app::emit!` for events
- Define `Error` enum with `thiserror::Error` and `Serialize` for proper error handling
- Custom `init()` creates collections explicitly (not `Default`)

### Event Handling Pattern

- ✅ DO: Define events with `#[app::event]` and use `app::emit!` macro
- ✅ DO: See `kv-store-with-handlers/src/lib.rs` for event handlers

```rust
#[app::event]
pub enum Event<'a> {
    Inserted { key: &'a str, value: &'a str },
    Updated { key: &'a str, value: &'a str },
    Removed { key: &'a str },
}

// Emit events in methods
pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
    if self.items.contains(&key)? {
        app::emit!(Event::Updated { key: &key, value: &value });
    } else {
        app::emit!(Event::Inserted { key: &key, value: &value });
    }
    // ...
}

// Event handlers (optional, see kv-store-with-handlers)
pub fn insert_handler(&mut self, key: &str, value: &str) -> app::Result<()> {
    app::log!("Handler called for insert: {} = {}", key, value);
    Ok(())
}
```

### Error Handling Pattern

- ✅ DO: Use `thiserror::Error` with `Serialize` for app errors
- ✅ DO: Return `app::Result<T>` and use `app::bail!` for errors

```rust
use thiserror::Error;
use calimero_sdk::serde::Serialize;

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("key not found: {0}")]
    NotFound(&'a str),
    #[error("invalid value: {0}")]
    InvalidValue(&'a str),
}

// Use in methods
pub fn get_result(&self, key: &str) -> app::Result<String> {
    let Some(value) = self.get(key)? else {
        app::bail!(Error::NotFound(key));
    };
    Ok(value)
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
thiserror = { workspace = true }  # For error handling

[build-dependencies]
calimero-wasm-abi = { workspace = true }  # For ABI generation
serde_json = { workspace = true }

[profile.release]
codegen-units = 1
opt-level = "z"
lto = true
debug = false
panic = "abort"
overflow-checks = true
```

**Note:** Workspace dependencies (`workspace = true`) are preferred in monorepo setups.

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

# Find event definitions
rg -n "#\[app::event\]" */src/

# Find event emissions
rg -n "app::emit!" */src/

# Find error definitions
rg -n "#\[derive.*Error" */src/

# Find error handling
rg -n "app::bail!" */src/
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
- Use `#[borsh(crate = "calimero_sdk::borsh")]` for borsh derives
- Use `#[serde(crate = "calimero_sdk::serde")]` for serde derives
- Return `app::Result<T>` from methods, not plain `T` or `Option<T>`
- Use nested CRDTs (`UnorderedMap<String, LwwRegister<String>>`) for last-write-wins semantics
- Events must use lifetime parameters: `Event<'a>` with `emits = for<'a> Event<'a>`
- Errors must implement `Serialize` for proper JSON-RPC error responses
- Use `--release` or `--profile app-release` for deployment
- Panic behavior in WASM differs from native
- Test locally with `meroctl call` before deployment
