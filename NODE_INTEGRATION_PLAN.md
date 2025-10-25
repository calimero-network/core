# Node Integration Plan for Tombstone Deletion & Full Resync

## Architecture Clarification

### Two Separate Storage Systems

**1. WASM Application Storage (`crates/storage/`)**
- Used BY WASM applications to store their state
- Accessed via SDK (`calimero_sdk::env::storage_*`)
- Applications use normal CRUD operations
- **Applications do NOT iterate their storage**

**2. Node Persistent Storage (`crates/store/` + `crates/store-rocksdb/`)**
- Used BY the node to persist contexts, applications, blocks, etc.
- Direct RocksDB access
- Node can iterate all keys efficiently
- **This is where GC and snapshots happen**

### Key Insight

The `crates/storage/` layer is an **abstraction** used by WASM apps. The actual data is stored in the node's RocksDB under specific prefixes (e.g., by context ID). The node has full access to RocksDB and can iterate all keys for any context.

---

## Correct Integration Approach

### 1. Node-Level Garbage Collection

**Location:** `crates/node/src/services/gc.rs` (NEW)

```rust
/// Garbage collection service for tombstone cleanup
pub struct GarbageCollector {
    store: Arc<dyn Store>,
    interval: Duration,
}

impl GarbageCollector {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self {
            store,
            interval: Duration::from_secs(12 * 3600), // 12 hours
        }
    }
    
    pub async fn run(self) {
        let mut interval = tokio::time::interval(self.interval);
        
        loop {
            interval.tick().await;
            
            if let Err(e) = self.collect_garbage().await {
                error!("Garbage collection failed: {}", e);
            }
        }
    }
    
    async fn collect_garbage(&self) -> Result<(), Error> {
        // Iterate all contexts
        for context_id in self.store.list_contexts()? {
            self.collect_for_context(&context_id).await?;
        }
        Ok(())
    }
    
    async fn collect_for_context(&self, context_id: &ContextId) -> Result<(), Error> {
        let retention = TOMBSTONE_RETENTION_NANOS;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        
        // Iterate all keys for this context
        let prefix = format!("context:{}:storage:", context_id);
        let mut keys_to_delete = Vec::new();
        
        for (key, value) in self.store.scan_prefix(&prefix)? {
            // Deserialize to check if it's a tombstone
            if let Ok(index) = borsh::from_slice::<EntityIndex>(&value) {
                if let Some(deleted_at) = index.deleted_at {
                    if now.saturating_sub(deleted_at) > retention {
                        keys_to_delete.push(key);
                    }
                }
            }
        }
        
        // Delete old tombstones
        for key in keys_to_delete {
            self.store.delete(&key)?;
        }
        
        Ok(())
    }
}
```

**Integration:**
- Add to `crates/node/src/lib.rs`
- Start as background task in node startup
- Configure interval via `NodeConfig`

---

### 2. Node-Level Snapshot Generation

**Location:** `crates/node/src/sync/snapshot.rs` (UPDATE)

```rust
/// Generate a snapshot of context storage for full resync
pub async fn generate_snapshot(
    store: &dyn Store,
    context_id: &ContextId,
) -> Result<Snapshot, Error> {
    let mut entities = Vec::new();
    let mut indexes = Vec::new();
    
    // Iterate all storage keys for this context
    let prefix = format!("context:{}:storage:", context_id);
    
    for (key, value) in store.scan_prefix(&prefix)? {
        // Parse key type (Index vs Entry)
        if is_index_key(&key) {
            let index: EntityIndex = borsh::from_slice(&value)?;
            
            // Skip tombstones in snapshots
            if index.deleted_at.is_none() {
                indexes.push((parse_id_from_key(&key), index));
            }
        } else if is_entry_key(&key) {
            let element: Element = borsh::from_slice(&value)?;
            entities.push((parse_id_from_key(&key), element));
        }
    }
    
    Ok(Snapshot {
        context_id: *context_id,
        entities,
        indexes,
        entity_count: entities.len(),
        timestamp: current_time_nanos(),
    })
}
```

---

### 3. DeleteRef Action Handling in Sync

**Location:** `crates/node/src/sync/actions.rs` (UPDATE)

The sync manager already handles `Action` propagation. We just need to ensure `Action::DeleteRef` is properly handled:

```rust
// In sync action handler
match action {
    Action::Create { .. } => { /* existing */ },
    Action::Update { .. } => { /* existing */ },
    Action::Delete { .. } => { /* existing */ },
    
    // NEW: Handle DeleteRef efficiently
    Action::DeleteRef { id, deleted_at } => {
        // Write to node storage
        let key = format!("context:{}:storage:index:{}", context_id, id);
        
        // Load existing index
        if let Some(data) = store.get(&key)? {
            let mut index: EntityIndex = borsh::from_slice(&data)?;
            
            // CRDT conflict resolution: only if remote is newer
            if let Some(local_deleted) = index.deleted_at {
                if deleted_at <= local_deleted {
                    return Ok(()); // Local deletion is newer, ignore
                }
            }
            
            // Apply deletion
            index.deleted_at = Some(deleted_at);
            store.put(&key, &borsh::to_vec(&index)?)?;
        }
        
        Ok(())
    },
    
    // ... other actions
}
```

---

### 4. Full Resync Protocol

**Location:** `crates/node/src/sync/resync.rs` (NEW)

```rust
/// Perform full resync for a context
pub async fn full_resync(
    store: &dyn Store,
    context_id: &ContextId,
    peer_id: &PeerId,
) -> Result<(), Error> {
    // 1. Request snapshot from peer
    let snapshot = request_snapshot_from_peer(peer_id, context_id).await?;
    
    // 2. Validate snapshot
    validate_snapshot(&snapshot)?;
    
    // 3. Clear local storage for this context (except sync state)
    clear_context_storage(store, context_id).await?;
    
    // 4. Apply snapshot
    apply_snapshot(store, context_id, &snapshot).await?;
    
    // 5. Update sync state
    let sync_key = format!("context:{}:sync_state:{}", context_id, peer_id);
    let sync_state = SyncState {
        peer_id: *peer_id,
        last_sync: current_time_nanos(),
    };
    store.put(&sync_key, &borsh::to_vec(&sync_state)?)?;
    
    Ok(())
}

async fn request_snapshot_from_peer(
    peer_id: &PeerId,
    context_id: &ContextId,
) -> Result<Snapshot, Error> {
    // Send snapshot request via network layer
    let request = SnapshotRequest { context_id: *context_id };
    let response = network::send_request(peer_id, request).await?;
    Ok(response.snapshot)
}

async fn apply_snapshot(
    store: &dyn Store,
    context_id: &ContextId,
    snapshot: &Snapshot,
) -> Result<(), Error> {
    // Write all entities
    for (id, element) in &snapshot.entities {
        let key = format!("context:{}:storage:entry:{}", context_id, id);
        store.put(&key, &borsh::to_vec(element)?)?;
    }
    
    // Write all indexes
    for (id, index) in &snapshot.indexes {
        let key = format!("context:{}:storage:index:{}", context_id, id);
        store.put(&key, &borsh::to_vec(index)?)?;
    }
    
    Ok(())
}
```

---

## Implementation Steps

### Phase 1: Basic GC (Week 1)

1. **Create GC Service**
   - File: `crates/node/src/services/gc.rs`
   - Iterate RocksDB by context prefix
   - Delete old tombstones
   - Add metrics (tombstones_collected, gc_duration_ms)

2. **Integrate into Node**
   - Update `crates/node/src/lib.rs`
   - Start GC task in `Node::start()`
   - Add config option for GC interval

3. **Testing**
   - Unit tests for GC logic
   - Integration test: create tombstones, wait, verify cleanup

### Phase 2: Snapshot Generation (Week 2)

4. **Implement Snapshot Generation**
   - File: `crates/node/src/sync/snapshot.rs`
   - Iterate RocksDB for context
   - Serialize to `Snapshot` struct
   - Exclude tombstones

5. **Network Protocol**
   - Add `SnapshotRequest` message
   - Add `SnapshotResponse` message
   - Handle in network layer

6. **Testing**
   - Generate snapshot, verify contents
   - Test tombstone exclusion

### Phase 3: Full Resync (Week 3)

7. **Implement Full Resync**
   - File: `crates/node/src/sync/resync.rs`
   - Request snapshot from peer
   - Clear local storage
   - Apply snapshot
   - Update sync state

8. **Sync State Tracking**
   - Track last sync time per peer
   - Detect when full resync needed (>2 days)
   - Automatic fallback to full resync

9. **Testing**
   - End-to-end resync test
   - Split-brain recovery test
   - Network partition recovery test

### Phase 4: DeleteRef Integration (Week 4)

10. **Update Sync Action Handling**
    - File: `crates/node/src/sync/actions.rs`
    - Handle `Action::DeleteRef`
    - CRDT conflict resolution
    - Broadcast on local deletion

11. **Testing**
    - Test DeleteRef propagation
    - Test conflict resolution (delete vs update)
    - Test out-of-order delivery

---

## Why This Approach is Correct

### ✅ Separation of Concerns
- WASM apps: simple CRUD, no GC concerns
- Node: manages storage lifecycle, GC, sync

### ✅ Performance
- RocksDB iteration is fast (C++ native)
- No WASM overhead for GC
- Efficient prefix scans

### ✅ Security
- WASM apps can't delete others' data
- GC only runs with node privileges
- Controlled by node config

### ✅ Scalability
- GC runs per-context (parallelizable)
- Doesn't block WASM execution
- Can rate-limit to avoid spikes

---

## MockedStorage vs MainStorage

### `MockedStorage` (for tests)
- Implements `IterableStorage` ✅
- Used in `crates/storage/` tests
- Has in-memory key iteration

### `MainStorage` (for WASM apps)
- Does NOT implement `IterableStorage` ✅
- WASM apps shouldn't iterate
- Node iterates RocksDB directly

This is correct ISP (Interface Segregation Principle) - don't force clients to depend on methods they don't use.

---

## Next Actions

1. Cancel SDK/runtime iteration work (DONE)
2. Focus on node-level GC service
3. Implement RocksDB prefix scanning
4. Add GC metrics and monitoring
5. Integrate into node startup

This architecture is clean, performant, and follows proper separation of concerns!

