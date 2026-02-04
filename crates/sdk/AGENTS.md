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
        app::emit!(Event::Inserted { key: &key, value: &value });
        self.items.insert(key, value.into())?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        app::log!("Getting key: {:?}", key);
        Ok(self.items.get(key)?.map(|v| v.get().clone()))
    }
}
```

### State Macro

Marks a struct as application state with CRDT support:

```rust
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::{Counter, LwwRegister, UnorderedMap};

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct AppState {
    counter: Counter,
    settings: UnorderedMap<String, LwwRegister<String>>,  // Use LwwRegister for last-write-wins
}

#[app::event]
pub enum Event<'a> {
    CounterIncremented,
    SettingChanged { key: &'a str },
}
```

**Key points:**

- Use `#[app::state(emits = for<'a> Event<'a>)]` to declare events
- Use nested CRDTs: `UnorderedMap<String, LwwRegister<String>>` for last-write-wins semantics
- Use `#[borsh(crate = "calimero_sdk::borsh")]` for borsh derives
- Define `Event<'a>` enum with `#[app::event]` for event handling

## Patterns

### Basic Application

- ✅ DO: Follow pattern in `apps/kv-store/src/lib.rs`

```rust
// apps/kv-store/src/lib.rs
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
            app::emit!(Event::Updated { key: &key, value: &value });
        } else {
            app::emit!(Event::Inserted { key: &key, value: &value });
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

- Return `app::Result<T>` from methods (not plain `T` or `Option<T>`)
- Use `value.into()` to convert `String` to `LwwRegister<String>` when inserting
- Use `?` operator for error propagation from CRDT operations
- Use `app::log!` for logging and `app::emit!` for events
- Extract values from `LwwRegister` with `.get().clone()`

### Environment Access

```rust
use calimero_sdk::app;

// Get context ID
let context_id = app::context_id();

// Get executor public key
let executor = app::executor_id();
```

### Event Emission

```rust
use calimero_sdk::app;

#[app::event]
pub enum Event<'a> {
    Inserted { key: &'a str, value: &'a str },
    Updated { key: &'a str, value: &'a str },
}

// Emit events using the macro
pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
    app::emit!(Event::Inserted { key: &key, value: &value });
    // ...
    Ok(())
}
```

### Error Handling

```rust
use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use thiserror::Error;

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

### CRDT Operations

```rust
use calimero_storage::collections::{LwwRegister, UnorderedMap};

// Insert with LwwRegister wrapping
self.items.insert(key, value.into())?;

// Get and extract value
let value = self.items.get(key)?.map(|v| v.get().clone());

// Check existence
if self.items.contains(&key)? {
    // ...
}

// In-place mutation with get_mut
if let Some(mut v) = self.items.get_mut(&key)? {
    v.set(new_value.clone());
    // Automatically persisted when guard is dropped
}

// Entry API
let entry = self.items.entry(key.clone())?;
let val = entry.or_insert(LwwRegister::new(value))?;
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

# Find event definitions
rg -n "#\[app::event\]" ../apps/

# Find event emissions
rg -n "app::emit!" ../apps/

# Find error definitions
rg -n "#\[derive.*Error" ../apps/

# Find error handling
rg -n "app::bail!" ../apps/

# Find CRDT usage patterns
rg -n "\.into\(\)" ../apps/

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
- Use `#[borsh(crate = "calimero_sdk::borsh")]` for borsh derives
- Use `#[serde(crate = "calimero_sdk::serde")]` for serde derives
- `#[app::init]` is required for state initialization
- Return `app::Result<T>` from methods, not plain `T` or `Option<T>`
- Use nested CRDTs (`UnorderedMap<String, LwwRegister<String>>`) for last-write-wins semantics
- Convert values with `.into()` when inserting: `self.items.insert(key, value.into())?`
- Extract values from `LwwRegister` with `.get().clone()`
- Use `?` operator for error propagation from CRDT operations
- Events must use lifetime parameters: `Event<'a>` with `emits = for<'a> Event<'a>`
- Errors must implement `Serialize` for proper JSON-RPC error responses
- Methods without `&mut self` are read-only (queries)
- Methods with `&mut self` can modify state (mutations)
- WASM apps must target `wasm32-unknown-unknown`
