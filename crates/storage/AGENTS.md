# calimero-storage - CRDT Collections

Conflict-free Replicated Data Types (CRDTs) for automatic conflict resolution in distributed state.

## Package Identity

- **Crate**: `calimero-storage`
- **Entry**: `src/lib.rs`
- **Framework**: borsh (serialization)

## Commands

```bash
# Build
cargo build -p calimero-storage

# Test
cargo test -p calimero-storage

# Test specific CRDT
cargo test -p calimero-storage test_counter -- --nocapture
```

## CRDT Types

| Type                | Purpose                  | Merge Strategy     |
| ------------------- | ------------------------ | ------------------ |
| `Counter`           | Distributed counter      | Increments sum     |
| `LwwRegister<T>`    | Last-write-wins register | Timestamp-based    |
| `UnorderedMap<K,V>` | Key-value map            | Entry-wise merge   |
| `Vector<T>`         | Ordered list             | Element-wise merge |
| `UnorderedSet<T>`   | Unique values            | Union              |

## File Organization

```
src/
├── lib.rs                    # Public exports
├── entities.rs               # Entity traits
├── interface.rs              # Storage interface
├── collections.rs            # Collections re-exports
├── collections/
│   ├── counter.rs            # Counter CRDT
│   ├── lww_register.rs       # Last-write-wins register
│   ├── unordered_map.rs      # Unordered map
│   ├── unordered_set.rs      # Unordered set
│   ├── vector.rs             # Vector CRDT
│   ├── rga.rs                # RGA (replicated growable array)
│   ├── root.rs               # Root collection
│   ├── nested.rs             # Nested CRDTs
│   ├── nested_map.rs         # Nested map
│   ├── frozen.rs             # Frozen collections
│   ├── frozen_value.rs       # Frozen value
│   ├── crdt_impls.rs         # CRDT implementations
│   ├── crdt_meta.rs          # CRDT metadata
│   ├── decompose_impls.rs    # Decompose implementations
│   ├── composite_key.rs      # Composite key
│   ├── user.rs               # User collection
│   ├── error.rs              # Collection errors
│   └── ...
├── address.rs                # Address types
├── integration.rs            # Integration utilities
├── action.rs                 # Actions
├── delta.rs                  # Delta handling
├── merge.rs                  # Merge logic
├── merge/
│   └── registry.rs           # Merge registry
├── snapshot.rs               # Snapshots
├── store.rs                  # Store
├── index.rs                  # Indexing
├── env.rs                    # Environment
├── js.rs                     # JS bindings
├── logical_clock.rs          # Logical clock
├── constants.rs              # Constants
├── error.rs                  # Errors
└── tests/
    ├── address.rs
    ├── collections.rs
    ├── crdt.rs
    ├── delta.rs
    ├── entities.rs
    ├── interface.rs
    ├── lww_register.rs
    ├── merge_integration.rs
    ├── rga.rs
    └── ...
```

## Patterns

### Using a CRDT

```rust
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::{LwwRegister, UnorderedMap};

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct AppState {
    data: UnorderedMap<String, LwwRegister<String>>,
}

#[app::event]
pub enum Event<'a> {
    Updated { key: &'a str },
}

#[app::logic]
impl AppState {
    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        self.data.insert(key, value.into())?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> app::Result<Option<String>> {
        Ok(self.data.get(key)?.map(|v| v.get().clone()))
    }
}
```

### Counter Pattern

```rust
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::Counter;

#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct CounterApp {
    count: Counter,
}

#[app::logic]
impl CounterApp {
    pub fn increment(&mut self) -> app::Result<()> {
        self.count.increment()?;
        Ok(())
    }

    pub fn value(&self) -> app::Result<u64> {
        Ok(self.count.value()?)
    }
}
```

### LwwRegister Pattern

```rust
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::LwwRegister;

#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct ConfigApp {
    setting: LwwRegister<String>,
}

#[app::logic]
impl ConfigApp {
    pub fn update(&mut self, value: String) -> app::Result<()> {
        self.setting.set(value);
        Ok(())
    }

    pub fn get(&self) -> app::Result<String> {
        Ok(self.setting.get().clone())
    }
}
```

## Key Files

| File                               | Purpose                |
| ---------------------------------- | ---------------------- |
| `src/lib.rs`                       | Public API             |
| `src/collections/counter.rs`       | Counter implementation |
| `src/collections/unordered_map.rs` | Map implementation     |
| `src/collections/lww_register.rs`  | LWW register           |
| `src/entities.rs`                  | Entity traits          |
| `src/interface.rs`                 | Storage interface      |

## JIT Index

```bash
# Find CRDT implementations
rg -n "impl.*Crdt" src/collections/

# Find merge logic
rg -n "fn merge" src/collections/

# Find collection traits
rg -n "pub trait" src/

# Find serialization
rg -n "BorshSerialize" src/collections/
```

## Storage Macros

Use `calimero-storage-macros` for derive macros:

```rust
use calimero_storage_macros::AtomicUnit;

#[derive(AtomicUnit)]
struct MyType {
    // fields
}
```

## Common Gotchas

- Use `#[app::state]` macro attribute
- CRDTs auto-merge on sync - no manual conflict resolution
- Use nested CRDTs (`UnorderedMap<String, LwwRegister<String>>`) for last-write-wins semantics
- Convert values with `.into()` when inserting: `self.data.insert(key, value.into())?`
- Extract values from `LwwRegister` with `.get().clone()`
- Return `app::Result<T>` from methods, not plain `T` or `Option<T>`
- Use `?` operator for error propagation from CRDT operations
- `UnorderedMap` keys must be unique per context
- `Vector` operations are position-based
- `Counter` only supports increment (no decrement by design)
- All CRDTs must be serializable with borsh
