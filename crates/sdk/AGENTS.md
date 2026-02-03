# calimero-sdk - App Development SDK

SDK for developing Calimero WebAssembly applications with macros and CRDT helpers.

## Package Identity

- **Crate**: `calimero-sdk`
- **Entry**: `src/lib.rs`
- **Sub-crates**: `calimero-sdk-macros`, `calimero-sdk-near`

## Commands

```bash
# Build
cargo build -p calimero-sdk

# Test (includes macro compile tests)
cargo test -p calimero-sdk
cargo test -p calimero-sdk-macros

# Build example app
cargo build -p kv-store --target wasm32-unknown-unknown --release
```

## File Organization

```
src/
├── lib.rs                    # Public exports
├── state.rs                  # State management
├── env.rs                    # Environment access
├── env/
│   └── ext.rs                # External functions
├── event.rs                  # Event handling
├── returns.rs                # Return types
├── types.rs                  # SDK types
├── macros.rs                 # Re-exported macros
└── private_storage.rs        # Private storage utilities
macros/
├── src/
│   ├── lib.rs                # Proc macro entry
│   ├── app.rs                # #[calimero_sdk::app] macro
│   ├── state.rs              # #[calimero_sdk::state] macro
│   └── ...
└── tests/
    └── ...                   # Compile-time tests (trybuild)
libs/near/                    # NEAR-specific SDK extensions
tests/
├── *.rs                      # Runtime tests
└── *.stderr                  # Expected compile errors
```

## Key Macros

### `#[calimero_sdk::app]`

Marks an impl block as the application entry point:

```rust
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::types::Error;
use calimero_sdk::app;

#[app::state]
#[derive(Default, BorshSerialize, BorshDeserialize)]
pub struct KvStore {
    data: UnorderedMap<String, String>,
}

#[app::logic]
impl KvStore {
    #[app::init]
    pub fn init() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: String, value: String) -> Result<(), Error> {
        self.data.insert(key, value);
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.data.get(key).cloned()
    }
}
```

### State Macro

Marks a struct as application state with CRDT support:

```rust
#[app::state]
#[derive(Default, BorshSerialize, BorshDeserialize)]
pub struct AppState {
    counter: Counter,
    settings: UnorderedMap<String, String>,
}
```

## Patterns

### Basic Application

- ✅ DO: Follow pattern in `apps/kv-store/src/lib.rs`

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

### Environment Access

```rust
use calimero_sdk::env;

// Get context ID
let context_id = env::context_id();

// Get executor public key
let executor = env::executor_id();
```

### Event Emission

```rust
use calimero_sdk::event;

#[derive(BorshSerialize)]
struct MyEvent {
    key: String,
    value: String,
}

event::emit(&MyEvent { key, value });
```

## Key Files

| File                       | Purpose                      |
| -------------------------- | ---------------------------- |
| `src/lib.rs`               | Public API re-exports        |
| `macros/src/lib.rs`        | Proc macro definitions       |
| `macros/src/app.rs`        | `#[app::*]` macro impl       |
| `src/env.rs`               | Environment functions        |
| `src/event.rs`             | Event emission               |
| `apps/kv-store/src/lib.rs` | Example app (best reference) |

## JIT Index

```bash
# Find macro implementations
rg -n "pub fn " macros/src/

# Find SDK public API
rg -n "pub " src/lib.rs

# Find example apps
rg -l "#\[app::state\]" ../apps/

# Find compile test expectations
ls tests/*.stderr
```

## Building Apps

```bash
# Add WASM target
rustup target add wasm32-unknown-unknown

# Build app
cargo build -p kv-store --target wasm32-unknown-unknown --release

# Output at: target/wasm32-unknown-unknown/release/kv_store.wasm
```

## Common Gotchas

- All state structs need `BorshSerialize` + `BorshDeserialize`
- `#[app::init]` is required for state initialization
- Methods without `&mut self` are read-only (queries)
- Methods with `&mut self` can modify state (mutations)
- WASM apps must target `wasm32-unknown-unknown`
