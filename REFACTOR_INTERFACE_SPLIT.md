# Refactoring Plan: Split interface.rs

## Current State

**File**: `crates/storage/src/interface.rs`  
**Size**: 1,277 lines  
**Problem**: Too large, hard to navigate, violates SRP

## Proposed Split

### 1. `action.rs` (~95 lines)
**Contains**:
- `Action` enum (Add, Update, DeleteRef, Compare)
- `ComparisonData` struct

**Why separate**: Actions are data types used across the module, natural to extract

### 2. `snapshot.rs` (~250 lines)
**Contains**:
- `Snapshot` struct
- `generate_snapshot()` - Lines 1122-1189
- `apply_snapshot()` - Lines 1190-1255
- `full_resync()` - Lines 1069-1121
- `clear_all_storage_except_sync_state()` - Lines 1256+
- Helper functions for snapshot operations

**Why separate**: All snapshot/resync operations are cohesive, used together, distinct from core CRUD

### 3. `sync_state.rs` (~50 lines)
**Contains**:
- `SyncState` struct (lines 214-227)
- `impl SyncState` (lines 228-257)
  - `new()`
  - `needs_full_resync()`
  - `update()`
- `get_sync_state()` helper
- `save_sync_state()` helper

**Why separate**: Sync state tracking is independent functionality

### 4. `interface.rs` (~880 lines remaining)
**Contains**:
- Module documentation
- `Interface<S>` struct
- Core CRUD operations:
  - `save()`
  - `add_child_to()`
  - `remove_child_from()`
  - `find_by_id()`
  - `children_of()`
  - `root()`
- Sync operations:
  - `apply_action()`
  - `compare_trees()`
  - `validate()`
- Internal helpers

**Why keep together**: These are the core Interface methods, work together, harder to split further

## File Structure After Refactoring

```
crates/storage/src/
├── interface/
│   ├── mod.rs           (main Interface, ~880 lines)
│   ├── action.rs        (Action enum, ~95 lines)
│   ├── snapshot.rs      (Snapshot operations, ~250 lines)
│   └── sync_state.rs    (SyncState tracking, ~50 lines)
├── interface.rs → DELETE (replaced by interface/ module)
├── index.rs
├── entities.rs
├── store.rs
└── ... other files
```

Or keep it simpler:

```
crates/storage/src/
├── interface.rs         (Interface + apply_action, ~700 lines)
├── action.rs           (Action enum, ~95 lines)
├── snapshot.rs         (Snapshot ops, ~250 lines)
├── sync_state.rs       (SyncState, ~50 lines)
├── comparison.rs       (compare_trees, ~180 lines)
└── ... other files
```

## Benefits

### Before
❌ 1,277 lines in one file  
❌ Hard to find specific functionality  
❌ Long compile times when changed  
❌ Difficult to review PRs  
❌ Violates Single Responsibility Principle  

### After
✅ ~4-5 focused files (~200-300 lines each)  
✅ Clear separation of concerns  
✅ Faster compile times (only changed file recompiles)  
✅ Easier to review  
✅ Better code organization  

## Recommended Approach

**Option A: Conservative Split** (Safest)
1. Extract `action.rs` - Just the Action enum
2. Extract `snapshot.rs` - All snapshot operations
3. Extract `sync_state.rs` - SyncState tracking
4. Keep rest in `interface.rs`

**Effort**: ~1 hour  
**Risk**: Low (clear boundaries)  
**Result**: interface.rs reduced to ~880 lines

**Option B: Aggressive Split** (More SRP)
1. Extract `action.rs` - Actions
2. Extract `snapshot.rs` - Snapshots
3. Extract `sync_state.rs` - SyncState
4. Extract `comparison.rs` - compare_trees and helpers
5. Extract `crud.rs` - save, add_child, remove_child, find_by_id
6. Keep only `apply_action` and core in `interface.rs`

**Effort**: ~2-3 hours  
**Risk**: Medium (more refactoring, more imports to manage)  
**Result**: No file > 300 lines

## Implementation Steps (Option A - Recommended)

### Step 1: Extract action.rs
```rust
// crates/storage/src/action.rs
pub enum Action { /* ... */ }
pub struct ComparisonData { /* ... */ }
```

### Step 2: Extract snapshot.rs
```rust
// crates/storage/src/snapshot.rs
use crate::interface::Interface;
use crate::store::{IterableStorage, StorageAdaptor};

pub struct Snapshot { /* ... */ }

impl<S: IterableStorage> Interface<S> {
    pub fn generate_snapshot() -> Result<Snapshot> { /* ... */ }
    pub fn apply_snapshot(snapshot: &Snapshot) -> Result<()> { /* ... */ }
    fn clear_all_storage_except_sync_state() -> Result<()> { /* ... */ }
}

impl<S: StorageAdaptor> Interface<S> {
    pub fn full_resync(node_id: Id, snapshot: Snapshot) -> Result<()> { /* ... */ }
}
```

### Step 3: Extract sync_state.rs
```rust
// crates/storage/src/sync_state.rs
pub struct SyncState { /* ... */ }

impl SyncState {
    pub fn new(node_id: Id) -> Self { /* ... */ }
    pub fn needs_full_resync(&self, retention: u64) -> bool { /* ... */ }
    pub fn update(&mut self, root_hash: [u8; 32]) { /* ... */ }
}

// Helper functions
pub fn get_sync_state<S: StorageAdaptor>(node_id: Id) -> Result<Option<SyncState>> { /* ... */ }
pub fn save_sync_state<S: StorageAdaptor>(state: &SyncState) -> Result<()> { /* ... */ }
```

### Step 4: Update interface.rs
```rust
// crates/storage/src/interface.rs
mod action;
mod snapshot;
mod sync_state;

pub use action::{Action, ComparisonData};
pub use snapshot::Snapshot;
pub use sync_state::SyncState;

// Keep Interface and core methods
```

### Step 5: Update lib.rs
```rust
// crates/storage/src/lib.rs
pub mod interface;

// Re-export key types at crate root for convenience
pub use interface::{Action, ComparisonData, Interface, Snapshot, SyncState};
```

## Migration Impact

**Breaking changes**: None  
**Public API**: Unchanged (re-exports maintain compatibility)  
**Tests**: Need import updates  
**Compile time**: Improved (incremental compilation benefits)

## Testing Strategy

1. Split one file at a time
2. Run tests after each split
3. Verify all 106 tests still pass
4. Check compilation of dependent crates

## Recommendation

**Do Option A now** (1 hour):
- Clear, safe boundaries
- Immediate benefit (880 vs 1277 lines)
- Low risk of breaking anything

**Consider Option B later** if needed:
- Further split Interface methods
- Could be done in a follow-up PR
- Requires more careful design

Want me to implement Option A (the conservative split)?

