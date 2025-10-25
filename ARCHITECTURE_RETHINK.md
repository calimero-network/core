# Architecture Rethink: Storage vs Sync vs State

## Current Confusion

We have mixed concerns:
- Storage primitives (keys, adapters, indexes)
- CRDT state operations (save, find, children)
- Synchronization (actions, comparison, snapshots)
- WASM interface (what apps actually use)

## Four Distinct Concerns

### 1. **WASM Local State** (what apps use directly)
**What**: CRUD operations within WASM
**Examples**:
- `app::state::save(&data)`
- `app::state::load()`
- `collection.add(item)`
- `collection.remove(item)`

**Where**: Should be in SDK or a thin wrapper

### 2. **State Mutations from Node** (applying remote changes)
**What**: Receiving and applying changes from other nodes
**Examples**:
- `apply_action(Action::Add { ... })`
- `apply_action(Action::Update { ... })`
- `apply_action(Action::DeleteRef { ... })`
- CRDT conflict resolution

**Where**: This is sync logic

### 3. **Merkle Tree Comparison** (delta sync)
**What**: Finding differences between two nodes
**Examples**:
- `compare_trees(local, remote)`
- Generate diff actions
- Recursive tree traversal

**Where**: This is sync logic

### 4. **Full DB Sync** (snapshot-based)
**What**: Complete state transfer
**Examples**:
- `generate_snapshot()`
- `apply_snapshot()`
- `full_resync()`

**Where**: This is sync logic

## Proposed Architecture

### Option A: Keep It Simple (Recommended)

```
calimero-storage/
  - Primitives: keys, adapters, indexes, entities
  - Collections: vector, unordered_set, unordered_map, root
  - Low-level only, no CRDT logic

calimero-sync/
  - Interface: High-level CRDT operations (save, add_child, etc.)
  - Actions: Action enum (Add, Update, DeleteRef, Compare)
  - Live sync: Real-time action broadcasting
  - Delta sync: Merkle tree comparison (compare_trees)
  - Full sync: Snapshots (generate/apply/resync)
  - State tracking: SyncState
```

**Benefits**:
- Clear separation: storage = primitives, sync = CRDT
- Everything sync-related in one place
- Easy to find functionality

### Option B: More Granular

```
calimero-storage/
  - Primitives only
  
calimero-state/
  - Interface: CRUD operations (save, find, add_child, remove_child)
  - Used by WASM apps
  - No network/sync knowledge
  
calimero-sync/
  - Actions: Action enum
  - Apply: apply_action() with CRDT logic
  - Compare: compare_trees() for delta sync
  - Snapshot: generate/apply for full sync
  - State: SyncState tracking
```

**Benefits**:
- Cleaner: apps depend on `state`, nodes depend on `sync`
- Better separation of concerns
- State layer is network-agnostic

### Option C: By Sync Strategy

```
calimero-storage/
  - Storage primitives
  - Interface: CRUD operations
  
calimero-sync/
  - action.rs: Action types
  - live.rs: Real-time broadcasting
  - delta.rs: Merkle tree comparison
  - full.rs: Snapshot-based resync
  - state.rs: Sync state tracking
  - apply.rs: apply_action() logic
```

**Benefits**:
- Organized by sync strategy
- Each module is focused
- Easy to understand different sync modes

## What Each Layer Should Contain

### calimero-storage (Primitives)
```rust
// Low-level storage operations
- Address, Id, Path
- Key enum (Index, Entry, SyncState)
- StorageAdaptor trait
- IterableStorage trait
- Index management (hashes, parent/child links)
- EntityIndex struct
- Entities (Element, Metadata, ChildInfo)
- Collections (Vector, UnorderedSet, etc.) - BASIC only
```

### calimero-sync (CRDT + Sync)
```rust
// High-level CRDT state management
- Interface: save(), find_by_id(), add_child_to(), remove_child_from()
- Action: Add, Update, DeleteRef, Compare
- apply_action(): CRDT conflict resolution
- compare_trees(): Merkle comparison
- Snapshot: full resync operations
- SyncState: track sync with peers
- Live/Delta/Full: three sync strategies
```

### What About Collections?

Collections (Vector, UnorderedSet, Root) are tricky:
- They use Interface methods (save, add_child, etc.)
- They're used by WASM apps
- They have sync concerns (Root::sync)

**Options**:
1. Keep in storage (basic operations only)
2. Move to sync (they use Interface)
3. Keep in storage, make them use sync crate

## My Recommendation

**Go with Option A + small tweak**:

```
calimero-storage/
  - address, entities, env, error
  - store (adapters, keys)
  - index (hash management)
  - integration (comparison utilities)
  - constants (retention periods)
  
calimero-sync/
  - action: Action enum, ComparisonData
  - interface: Interface<S> with all CRDT operations
  - live: Real-time action broadcasting (push_action, commit_root)
  - delta: Merkle tree comparison (compare_trees - extracted from Interface)
  - full: Snapshot operations (generate, apply, resync)
  - state: SyncState tracking
  
calimero-storage/collections/
  - Keep collections in storage
  - They import Interface from calimero-sync
  - calimero-storage depends on calimero-sync for high-level ops
```

Wait, that creates circular dependency! Let me fix:

```
calimero-storage/
  - Primitives only (no Interface, no sync)
  - address, entities, env, error, store, index, integration, constants
  
calimero-sync/
  - interface: Interface<S> (uses storage primitives)
  - action, live, delta, full, state
  - collections: Vector, UnorderedSet, Root (moved here!)
```

This way:
- storage = primitives (no deps on sync)
- sync = CRDT logic + collections (depends on storage)
- Clean dependency graph

Does this make sense?

