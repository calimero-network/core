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

# Test merge dispatch
cargo test -p calimero-storage merge_dispatch -- --nocapture
```

## CRDT Types and Merge Strategies

| Type                       | Purpose                  | Merge Strategy                    | Storage    |
| -------------------------- | ------------------------ | --------------------------------- | ---------- |
| `GCounter`                 | Grow-only counter        | Max per executor                  | Blob       |
| `PnCounter`                | Positive-negative counter| Max per executor (pos & neg maps) | Blob       |
| `LwwRegister<T>`           | Last-write-wins register | Timestamp-based (later wins)      | Blob       |
| `ReplicatedGrowableArray`  | Collaborative text (RGA) | Union of characters               | Blob       |
| `UnorderedMap<K,V>`        | Key-value map            | Entry-wise merge*                 | Structured |
| `UnorderedSet<T>`          | Unique values            | Union (add-wins)                  | Structured |
| `Vector<T>`                | Ordered list             | Element-wise merge*               | Structured |
| `UserStorage`              | Per-user data            | LWW per user                      | Blob       |
| `FrozenStorage`            | Immutable data           | First-write-wins                  | Blob       |

*Structured storage: Entries are separate entities with their own CrdtType, merged individually.

## AI Agent Mental Model: CRDT Merge Architecture

### Two Merge Contexts (Critical for Understanding)

The storage system has **two different merge contexts**:

#### Context A: Non-Root Entity Sync (try_merge_non_root)

When individual entities (map entries, vector elements, etc.) are synced:

```
Entity Conflict -> Has CrdtType? -> is_builtin_crdt()? -> merge_by_crdt_type()
                       |                  |
                       No                 No (Custom only)
                       v                  v
                    LWW fallback      WASM callback (PR #1940)
```

**Key insight**: Collections (UnorderedMap, Vector, UnorderedSet) return incoming at
container-level because entries are stored as **separate entities** - each entry merges
with its own CrdtType.

#### Context B: Root Entity Sync (merge_root_state)

When the entire app state (root entity) conflicts:

```
Root Conflict -> Try merge registry (Mergeable trait) -> Fallback LWW
```

The Mergeable trait implementations in crdt_impls.rs provide **recursive merge**:
- UnorderedMap::merge() iterates entries and calls value.merge(&other_value)
- Vector::merge() merges elements at same indices recursively
- This is where nested CRDT merging happens!

### Merge Decision Tree (Corrected)

```
+---------------------------------------------------------------------+
|                    ENTITY CONFLICT DETECTED                         |
|               (existing.updated_at <= incoming.updated_at)          |
+-------------------------------+-------------------------------------+
                                |
                    +-----------+-----------+
                    |   Is ROOT entity?     |
                    +-----------+-----------+
                                |
            +-------Yes---------+----------No-----------+
            |                   |                       |
            v                   |                       v
+-----------------------+       |       +-------------------------+
|  merge_root_state()   |       |       |  Has CrdtType metadata? |
|  1. Try merge registry|       |       +-----------+-------------+
|     (Mergeable trait) |       |                   |
|  2. Fallback to LWW   |       |           +---No--+---Yes---+
+-----------------------+       |           |                 |
                                |           v                 v
                                |      LWW fallback    is_builtin_crdt()?
                                |      (legacy data)          |
                                |                     +---Yes-+---No---+
                                |                     |                |
                                |                     v                v
                                |          merge_by_crdt_type()   WASM callback
                                |                                 (or LWW fallback)
```

### merge_by_crdt_type() Dispatch Table

| CrdtType       | Function              | Behavior                              |
| -------------- | --------------------- | ------------------------------------- |
| `GCounter`     | `merge_g_counter()`   | Counter::merge() - max per executor   |
| `PnCounter`    | `merge_pn_counter()`  | Counter::merge() - max per executor   |
| `Rga`          | `merge_rga()`         | RGA::merge() - union characters       |
| `LwwRegister`  | Returns incoming      | Timestamp comparison done by caller   |
| `UnorderedMap` | Returns incoming      | Entries are separate entities*        |
| `UnorderedSet` | Returns incoming      | Entries are separate entities*        |
| `Vector`       | Returns incoming      | Entries are separate entities*        |
| `UserStorage`  | Returns incoming      | LWW per user                          |
| `FrozenStorage`| Returns existing      | First-write-wins (immutable)          |
| `Custom`       | WasmRequired error    | Needs app-defined merge via WASM      |

*These types use "Structured" storage - container metadata only; entries sync separately.

### is_builtin_crdt() Definition

```rust
pub fn is_builtin_crdt(crdt_type: &CrdtType) -> bool {
    !matches!(crdt_type, CrdtType::Custom(_))
}
```

**ALL variants except Custom are built-in!**

## Key Files for Merge Understanding

| File                              | Purpose                                    |
| --------------------------------- | ------------------------------------------ |
| `src/merge.rs`                    | merge_by_crdt_type(), merge_root_state()   |
| `src/merge/registry.rs`           | Merge registry for Mergeable types         |
| `src/interface.rs`                | try_merge_non_root(), save_internal()      |
| `src/collections/crdt_meta.rs`    | CrdtType, Mergeable, CrdtMeta traits       |
| `src/collections/crdt_impls.rs`   | Mergeable implementations for all CRDTs    |

## File Organization

```
src/
├── lib.rs                    # Public exports
├── entities.rs               # Entity traits
├── interface.rs              # Storage interface (merge dispatch here!)
├── merge.rs                  # merge_by_crdt_type(), merge_root_state()
├── merge/
│   └── registry.rs           # Merge registry for Mergeable types
├── collections.rs            # Collections re-exports
├── collections/
│   ├── crdt_meta.rs          # CrdtType, Mergeable, CrdtMeta traits
│   ├── crdt_impls.rs         # Mergeable implementations
│   ├── counter.rs            # GCounter/PnCounter CRDT
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
│   ├── decompose_impls.rs    # Decompose implementations
│   ├── composite_key.rs      # Composite key
│   ├── user.rs               # User collection
│   ├── error.rs              # Collection errors
│   └── ...
├── address.rs                # Address types
├── integration.rs            # Integration utilities
├── action.rs                 # Actions
├── delta.rs                  # Delta handling
├── snapshot.rs               # Snapshots
├── store.rs                  # Store adaptor
├── index.rs                  # Entity indexing (Merkle tree)
├── env.rs                    # RuntimeEnv (storage backend injection)
├── js.rs                     # JS bindings
├── logical_clock.rs          # HLC (Hybrid Logical Clock)
├── constants.rs              # Constants
├── error.rs                  # Errors
└── tests/
    ├── merge_dispatch.rs     # Tests for merge_by_crdt_type
    ├── merge_integration.rs  # Integration tests
    └── ...
```

## CIP Invariants (Sync Protocol)

When working with merge logic, these invariants MUST be preserved:

- **I5 (No Silent Data Loss)**: Built-in CRDT types MUST use their semantic merge rules,
  never be overwritten by LWW. GCounter contributions from different nodes must sum.
- **I10 (Metadata Persistence)**: crdt_type MUST be persisted in entity metadata for
  correct merge dispatch.

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
    count: Counter,  // GCounter by default (ALLOW_DECREMENT=false)
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

## JIT Index

```bash
# Find merge dispatch logic
rg -n "merge_by_crdt_type" src/

# Find Mergeable trait implementations
rg -n "impl.*Mergeable" src/collections/crdt_impls.rs

# Find CrdtType definitions
rg -n "pub enum CrdtType" src/collections/crdt_meta.rs

# Find non-root merge logic
rg -n "try_merge_non_root" src/interface.rs

# Find root merge logic
rg -n "merge_root_state" src/merge.rs

# Find CRDT implementations
rg -n "impl.*CrdtMeta" src/collections/

# Find collection traits
rg -n "pub trait" src/

# Find is_builtin_crdt
rg -n "is_builtin_crdt" src/merge.rs
```

## Storage Macros

Use calimero-storage-macros for derive macros:

```rust
use calimero_storage_macros::AtomicUnit;

#[derive(AtomicUnit)]
struct MyType {
    // fields
}
```

## Common Gotchas

- Use #[app::state] macro attribute - it auto-generates Mergeable impl
- CRDTs auto-merge on sync - no manual conflict resolution needed
- Use nested CRDTs (UnorderedMap<String, LwwRegister<String>>) for last-write-wins semantics
- Convert values with .into() when inserting: self.data.insert(key, value.into())?
- Extract values from LwwRegister with .get().clone()
- Return app::Result<T> from methods, not plain T or Option<T>
- Use ? operator for error propagation from CRDT operations
- UnorderedMap keys must be unique per context
- Vector operations are position-based
- Counter only supports increment by default (use PnCounter for decrement)
- All CRDTs must be serializable with borsh
- **Structured vs Blob storage**: Collections use structured storage (entries are separate
  entities), while counters and registers use blob storage (single serialized value)

## Further Documentation

- readme/architecture.md - Deep dive into three-layer conflict resolution
- readme/DOCUMENTATION_INDEX.md - Full documentation index
- src/merge.rs - Module-level docs on merge dispatch
- src/collections/crdt_impls.rs - Module-level docs on Mergeable implementations


## ⚠️ KNOWN ISSUE: Root Entity LWW Fallback (I5 Violation Risk)

**Status**: Needs fix (tracked for future work)

The `merge_root_state()` function falls back to LWW when no merge function is registered.
This can cause **silent data loss** if the root entity contains CRDTs (Counter, etc.)
and violates **Invariant I5 (No Silent Data Loss)**.

**Quick Reference:**

| Scenario | Protected? |
| -------- | ---------- |
| WASM app with `#[app::state]` | ✅ Yes (auto-registers) |
| WASM app without `#[app::state]` | ❌ No |
| Tests without `register_crdt_merge()` | ❌ No |

**Mitigation**: Always use `#[app::state]` macro for WASM apps.

👉 **See `readme/merging.md` section "KNOWN ISSUE: Root Entity LWW Fallback"** for:
- Detailed explanation and data loss example
- Complete affected scenarios table
- Proposed fix options
