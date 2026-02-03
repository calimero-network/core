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
├── collections/
│   ├── mod.rs                # Collections index
│   ├── counter.rs            # Counter CRDT
│   ├── lww_register.rs       # Last-write-wins register
│   ├── unordered_map.rs      # Unordered map
│   ├── unordered_set.rs      # Unordered set
│   └── vector.rs             # Vector CRDT
├── address/
│   └── ...                   # Address types
├── integration/
│   └── ...                   # Integration utilities
└── tests/
    └── ...
```

## Patterns

### Using a CRDT

```rust
use calimero_storage::collections::UnorderedMap;

#[calimero_sdk::state]
struct AppState {
    data: UnorderedMap<String, String>,
}

impl AppState {
    pub fn set(&mut self, key: String, value: String) {
        self.data.insert(key, value);
    }

    pub fn get(&self, key: &str) -> Option<&String> {
        self.data.get(key)
    }
}
```

### Counter Pattern

```rust
use calimero_storage::collections::Counter;

#[calimero_sdk::state]
struct CounterApp {
    count: Counter,
}

impl CounterApp {
    pub fn increment(&mut self) {
        self.count.increment();
    }

    pub fn value(&self) -> u64 {
        self.count.value()
    }
}
```

### LwwRegister Pattern

```rust
use calimero_storage::collections::LwwRegister;

#[calimero_sdk::state]
struct ConfigApp {
    setting: LwwRegister<String>,
}

impl ConfigApp {
    pub fn update(&mut self, value: String) {
        self.setting.set(value);
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

- CRDTs auto-merge on sync - no manual conflict resolution
- `UnorderedMap` keys must be unique per context
- `Vector` operations are position-based
- `Counter` only supports increment (no decrement by design)
- All CRDTs must be serializable with borsh
