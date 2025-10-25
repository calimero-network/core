# Architecture Decision: GC and Snapshots at Node Level, Not WASM Level

## Decision

**Garbage collection and snapshot generation will be implemented at the node level by directly iterating RocksDB, NOT exposed as WASM host functions.**

## Context

We need to implement:
1. Garbage collection of old tombstones (1-day retention)
2. Snapshot generation for full resync
3. Full resync protocol

Initial approach (WRONG):
- Add `storage_iter_keys()` host function
- Expose it to WASM via SDK
- Let WASM apps iterate their own storage

## Why the Initial Approach Was Wrong

### 1. **Separation of Concerns Violation**

WASM applications should focus on **business logic**, not storage lifecycle management:

```rust
// WASM apps should do THIS:
app::state::save(&my_data)?;
let data = app::state::load()?;

// NOT THIS:
let keys = env::storage_iter_keys(); // ❌ Why would an app need this?
for key in keys {
    // ... GC logic in WASM? No!
}
```

### 2. **Security Risk**

Exposing iteration to WASM opens attack vectors:
- WASM app could enumerate all keys (privacy leak)
- Could DOS by repeatedly iterating large datasets
- Could interfere with other apps' storage

### 3. **Performance**

- WASM iteration is slower (serialization overhead)
- Node can use native RocksDB iterators (C++)
- GC doesn't need WASM execution context

### 4. **Architectural Layering**

```
┌─────────────────────────────────────┐
│     WASM Application Layer          │  ← Business logic only
│  (crates/storage/ abstraction)      │  ← No iteration needed
└─────────────────────────────────────┘
            ↓ uses
┌─────────────────────────────────────┐
│   Runtime Host Functions            │  ← CRUD operations only
│  (storage_read, write, remove)      │  ← NO iteration
└─────────────────────────────────────┘
            ↓ calls
┌─────────────────────────────────────┐
│   Node Storage Layer                │  ← Full RocksDB access
│  (crates/store-rocksdb/)            │  ← Iteration happens HERE
└─────────────────────────────────────┘
```

## Correct Approach

### 1. Node-Level Garbage Collection

**File:** `crates/node/src/services/gc.rs`

```rust
pub struct GarbageCollector {
    store: Arc<RocksDB>,
    interval: Duration, // 12 hours
}

impl GarbageCollector {
    pub async fn run(self) {
        loop {
            tokio::time::sleep(self.interval).await;
            
            // Iterate all contexts
            for context_id in self.store.list_contexts()? {
                self.collect_for_context(&context_id).await?;
            }
        }
    }
    
    async fn collect_for_context(&self, context_id: &ContextId) -> Result<()> {
        // Direct RocksDB iteration
        let prefix = format!("context:{}:storage:index:", context_id);
        
        for (key, value) in self.store.scan_prefix(&prefix)? {
            let index: EntityIndex = borsh::from_slice(&value)?;
            
            if let Some(deleted_at) = index.deleted_at {
                if is_expired(deleted_at) {
                    self.store.delete(&key)?; // Direct RocksDB delete
                }
            }
        }
        
        Ok(())
    }
}
```

**Advantages:**
- ✅ Runs in background, doesn't block WASM
- ✅ Uses native RocksDB iterators (fast)
- ✅ Can be rate-limited to avoid load spikes
- ✅ Metrics and monitoring at node level
- ✅ WASM apps completely unaware of GC

### 2. Node-Level Snapshot Generation

**File:** `crates/node/src/sync/snapshot.rs`

```rust
pub async fn generate_snapshot(
    store: &RocksDB,
    context_id: &ContextId,
) -> Result<Snapshot> {
    let mut entities = Vec::new();
    let mut indexes = Vec::new();
    
    // Direct RocksDB iteration
    let prefix = format!("context:{}:storage:", context_id);
    
    for (key, value) in store.scan_prefix(&prefix)? {
        if is_index_key(&key) {
            let index: EntityIndex = borsh::from_slice(&value)?;
            if index.deleted_at.is_none() { // Skip tombstones
                indexes.push((parse_id(&key), index));
            }
        } else if is_entry_key(&key) {
            let element: Element = borsh::from_slice(&value)?;
            entities.push((parse_id(&key), element));
        }
    }
    
    Ok(Snapshot { entities, indexes, .. })
}
```

**Advantages:**
- ✅ Full control over snapshot contents
- ✅ Efficient bulk read from RocksDB
- ✅ Can compress before network transfer
- ✅ No WASM execution needed

### 3. Storage Abstraction Remains Simple

**WASM apps continue using the simple interface:**

```rust
// crates/storage/ - WASM application storage abstraction

pub trait StorageAdaptor {
    fn storage_read(key: Key) -> Option<Vec<u8>>;
    fn storage_write(key: Key, value: &[u8]) -> bool;
    fn storage_remove(key: Key) -> bool;
}

// No iteration exposed to WASM!

// Only MockedStorage (for tests) implements IterableStorage
pub trait IterableStorage: StorageAdaptor {
    fn storage_iter_keys() -> Vec<Key>; // Only for tests
}
```

## Data Flow

### WASM Application Write

```
WASM App
  ↓ app::state::save(&data)
crates/storage/interface.rs
  ↓ Interface::save()
crates/storage/store.rs (MainStorage)
  ↓ storage_write()
crates/sdk/env.rs
  ↓ env::storage_write()
crates/sys
  ↓ extern "C" storage_write()
Runtime Host Function
  ↓ VMLogic::storage.set()
Node Storage Layer
  ↓ RocksDB::put()
Disk 💾
```

### Node GC

```
Node GC Service (background task)
  ↓ RocksDB::scan_prefix()
  ↓ filter expired tombstones
  ↓ RocksDB::delete()
Disk 💾 (tombstones removed)
```

**Notice:** WASM is NOT involved in GC!

## Implementation Plan

### Week 1: Node GC Service
- [ ] Create `crates/node/src/services/gc.rs`
- [ ] Implement RocksDB prefix scanning
- [ ] Add tombstone expiration logic
- [ ] Integrate into node startup
- [ ] Add metrics (tombstones_collected, gc_duration_ms)

### Week 2: Snapshot Generation
- [ ] Create `crates/node/src/sync/snapshot.rs`
- [ ] Implement RocksDB-based snapshot generation
- [ ] Add network protocol for snapshot transfer
- [ ] Test snapshot serialization/deserialization

### Week 3: Full Resync
- [ ] Implement full resync protocol
- [ ] Add sync state tracking
- [ ] Automatic fallback to full resync (>2 days offline)
- [ ] End-to-end integration tests

### Week 4: DeleteRef Integration
- [ ] Update sync action handling for `Action::DeleteRef`
- [ ] CRDT conflict resolution
- [ ] Broadcast on local deletion
- [ ] Test out-of-order delivery

## Testing Strategy

### Unit Tests
- GC identifies expired tombstones correctly
- Snapshot excludes tombstones
- CRDT conflict resolution works

### Integration Tests
- Full GC cycle: create → wait → collect → verify
- Full resync: diverge → resync → verify identical state
- Network partition recovery

### Performance Tests
- GC handles 10K tombstones in <1s
- Snapshot generation for 100K entities in <5s
- No impact on WASM execution during GC

## Conclusion

**The correct architecture:**
- WASM apps: Simple CRUD, no iteration
- Node: Full storage lifecycle (GC, snapshots, sync)
- Clean separation of concerns
- Better performance and security

This follows proper layering and the principle that **infrastructure concerns (GC, sync) belong at the infrastructure layer (node), not at the application layer (WASM)**.

