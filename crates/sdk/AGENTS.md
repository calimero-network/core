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

### `#[app::migrate]` — state-migration export

Marks a stand-alone function as the WASM export the node runtime
calls during `upgrade_group(target=v2, migrate_method=...)`. The
function reads the old state via `calimero_sdk::state::read_raw()`,
constructs the new state struct, and returns it; the SDK macro
wraps it in the same `Root::new(...)` context as `#[app::init]` so
collection writes made inside the migrate body persist correctly.

```rust
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::state::read_raw;
use calimero_storage::collections::{LwwRegister, UnorderedMap};

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct AppV2 {
    items: UnorderedMap<String, LwwRegister<String>>,
    notes: LwwRegister<String>,  // new in v2
}

#[app::event]
pub enum Event<'a> {
    Migrated { from_version: &'a str, to_version: &'a str },
}

// Private borsh-deserialize shape matching the v1 state byte layout.
// Field order MUST match v1's `#[app::state]` struct (borsh is
// positional).
#[derive(BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct AppV1 {
    items: UnorderedMap<String, LwwRegister<String>>,
}

#[app::migrate]
pub fn migrate_v1_to_v2() -> AppV2 {
    let old_bytes = read_raw().unwrap_or_else(|| {
        panic!("Migration failed: no existing state.");
    });
    let old: AppV1 = BorshDeserialize::deserialize(&mut &old_bytes[..])
        .unwrap_or_else(|e| panic!("V1 deserialize: {e:?}"));

    app::emit!(Event::Migrated {
        from_version: "1.0.0",
        to_version: "2.0.0",
    });

    AppV2 {
        items: old.items,
        notes: LwwRegister::new("added in v2".to_owned()),
    }
}
```

**Key points:**

- Free function, not a method on the state struct. The return type
  is the v2 state struct, which determines the event emitter the
  macro registers.
- Read v1 state via `read_raw()` and deserialise into a private
  borsh-only shadow struct of the v1 layout. Don't import the v1
  crate's `#[app::state]` — it would pull in v1's full SDK surface.
- Carrying a collection across versions: `items: old.items` —
  the existing storage handle survives, no re-population needed.
- Creating a NEW collection in migrate (e.g. archiving a removed
  field into `UnorderedMap`): construct with
  `UnorderedMap::new_with_field_name("...")` and insert as normal.
  The SDK's macro wraps the migrate body in the same
  `Root::new + __assign_deterministic_ids + commit` flow as
  `#[app::init]`, so inserts persist to the same storage path a
  later `&self` read computes for the same field.
- Panic on unrecoverable inputs (corrupted state, deserialise
  failure) — matches the existing `migration-suite-v{2..5}-add-field`
  pattern. A panic traps the WASM and aborts the upgrade, leaving
  v1 state intact for retry.

End-to-end coverage of migration shapes lives in
`workflows/app-migration/` (per-context migration, namespace cascade,
12 schema-shape scenarios). See
`apps/migrations/migration-suite-v{1..5}` (chain fixtures) and
`apps/migrations/scenario-*-v{1,2}` (standalone scenario pairs) for
concrete reference implementations.

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
