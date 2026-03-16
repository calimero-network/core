# apps/ - Example WASM Applications

Example and test WebAssembly applications demonstrating Calimero SDK usage.

- **Target**: `wasm32-unknown-unknown`
- **SDK**: `calimero-sdk`
- **Storage**: `calimero-storage` (CRDTs)

## Build Commands

```bash
rustup target add wasm32-unknown-unknown   # one-time setup
cargo build -p kv-store --target wasm32-unknown-unknown --release
./scripts/build-all-apps.sh
cd apps/kv-store && ./build.sh
```

## Available Apps

| App | Purpose | Good Example For |
|---|---|---|
| `kv-store` | Simple key-value store | Basic CRDT usage ← start here |
| `kv-store-init` | KV with custom init | `#[app::init]` pattern |
| `kv-store-with-handlers` | KV with event handlers | Event handling |
| `access-control` | Permission management | Authorization patterns |
| `blobs` | Blob storage demo | Blob operations |
| `collaborative-editor` | Collaborative text | Complex CRDTs |
| `xcall-example` | Cross-context calls | XCall pattern |
| `demo-blockchain-integrations` | NEAR/ICP/Ethereum proposals | Blockchain E2E |
| `migrations/migration-suite-v1..v5` | Migration examples | Schema changes |

## App Structure

```
app-name/
├── Cargo.toml        # crate-type = ["cdylib"] required
├── build.sh          # ./build.sh runs the WASM build
├── build.rs          # ABI generation (optional)
├── src/lib.rs        # App implementation
└── workflows/*.yml   # merobox test workflows
```

## Canonical App Pattern

Reference: `kv-store/src/lib.rs`

```rust
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
    Updated  { key: &'a str, value: &'a str },
    Removed  { key: &'a str },
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
        KvStore { items: UnorderedMap::new() }
    }

    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        app::log!("set {:?} = {:?}", key, value);
        if self.items.contains(&key)? {
            app::emit!(Event::Updated { key: &key, value: &value });
        } else {
            app::emit!(Event::Inserted { key: &key, value: &value });
        }
        self.items.insert(key, value.into())?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }
}
```

## Cargo.toml Pattern

```toml
[package]
name = "kv-store"
version = "0.0.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]          # required for WASM

[dependencies]
calimero-sdk     = { workspace = true }
calimero-storage = { workspace = true }
thiserror        = { workspace = true }

[build-dependencies]
calimero-wasm-abi = { workspace = true }
serde_json        = { workspace = true }

[profile.release]
codegen-units = 1
opt-level     = "z"
lto           = true
debug         = false
panic         = "abort"
overflow-checks = true
```

## Key Rules

- `crate-type = ["cdylib"]` — mandatory
- State fields must be borsh-serializable
- Use `#[borsh(crate = "calimero_sdk::borsh")]` on borsh derives
- Use `#[serde(crate = "calimero_sdk::serde")]` on serde derives
- Return `app::Result<T>` from all methods — never plain `T` or `Option<T>`
- Mutations: `&mut self`; queries: `&self`
- CRDT insert: `value.into()` to wrap in `LwwRegister`
- CRDT read: `.get().clone()` to extract from `LwwRegister`
- Events require lifetimes: `Event<'a>` with `emits = for<'a> Event<'a>`
- Errors must impl `Serialize` for JSON-RPC error responses
- Use `app::bail!(Error::...)` instead of `return Err(...)`

## Quick Search

```bash
rg -n "#\[app::state\]" */src/
rg -n "UnorderedMap|Counter|LwwRegister|Vector" */src/
rg -n "#\[app::event\]" */src/
rg -n "app::emit!" */src/
rg -n "app::bail!" */src/
```
