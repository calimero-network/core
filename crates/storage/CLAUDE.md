# calimero-storage - CRDT Collections

Conflict-free Replicated Data Types for automatic conflict resolution in distributed state.

- **Crate**: `calimero-storage`
- **Entry**: `src/lib.rs`
- **Serialization**: borsh

## Build & Test

```bash
cargo build -p calimero-storage
cargo test -p calimero-storage
cargo test -p calimero-storage test_counter -- --nocapture
```

## CRDT Types

| Type | Purpose | Merge Strategy |
|---|---|---|
| `Counter` | Distributed counter | Sum of increments |
| `LwwRegister<T>` | Last-write-wins value | Timestamp-based |
| `UnorderedMap<K,V>` | Key-value map | Entry-wise merge |
| `Vector<T>` | Ordered list | Element-wise merge |
| `UnorderedSet<T>` | Unique values | Union |

## File Layout

```
src/
├── lib.rs
├── collections/
│   ├── counter.rs
│   ├── lww_register.rs
│   ├── unordered_map.rs
│   ├── unordered_set.rs
│   ├── vector.rs
│   ├── nested.rs / nested_map.rs
│   ├── frozen.rs / frozen_value.rs
│   └── ...
├── entities.rs      # Entity traits
├── interface.rs     # Storage interface
├── delta.rs         # Delta handling
├── merge.rs         # Merge logic
├── logical_clock.rs
└── tests/
```

## Usage Patterns

### LwwRegister in a Map (most common)

```rust
use calimero_storage::collections::{LwwRegister, UnorderedMap};

// In state struct
items: UnorderedMap<String, LwwRegister<String>>

// Insert
self.items.insert(key, value.into())?;

// Read
let val = self.items.get(key)?.map(|v| v.get().clone());

// Check existence
if self.items.contains(&key)? { ... }

// In-place mutation (auto-persisted on drop)
if let Some(mut v) = self.items.get_mut(&key)? {
    v.set(new_value);
}

// Entry API
let entry = self.items.entry(key.clone())?;
let val = entry.or_insert(LwwRegister::new(value))?;
```

### Counter

```rust
use calimero_storage::collections::Counter;

self.count.increment()?;
let n = self.count.value()?;
```

### Storage Macros

```rust
use calimero_storage_macros::AtomicUnit;

#[derive(AtomicUnit)]
struct MyType { /* fields */ }
```

## Key Files

| File | Purpose |
|---|---|
| `src/lib.rs` | Public API |
| `src/collections/unordered_map.rs` | Map implementation |
| `src/collections/lww_register.rs` | LWW register |
| `src/collections/counter.rs` | Counter |
| `src/entities.rs` | Entity traits |
| `src/interface.rs` | Storage interface |

## Quick Search

```bash
rg -n "impl.*Crdt" src/collections/
rg -n "fn merge" src/collections/
rg -n "pub trait" src/
```

## Gotchas

- CRDTs auto-merge on sync — no manual conflict resolution needed
- `Counter` only increments (no decrement by design)
- All CRDT types must be borsh-serializable
- `UnorderedMap` keys must be unique per context
- `Vector` operations are position-based
- `.into()` converts `T` → `LwwRegister<T>` on insert
- `.get().clone()` extracts `T` from `LwwRegister<T>` on read
