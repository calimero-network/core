# Storage Integration Roadmap

This document outlines the tasks required to integrate the new tombstone deletion and full resync features into the Calimero node.

## Status Legend
- 🟢 **Ready** - Implementation complete, can be integrated
- 🟡 **Partial** - Core implementation done, needs integration work
- 🔴 **Blocked** - Waiting on dependencies
- ⚪ **Not Started** - Planning only

---

## Phase 1: Backend Storage Support (High Priority)

### 1.1 Implement `storage_iter_keys()` for Production Storage
**Status:** 🔴 Blocked (needs RocksDB/backend work)
**Priority:** Critical
**Dependencies:** None
**Estimated Effort:** 2-3 days

**Tasks:**
- [ ] Check which storage backend the node uses (likely RocksDB)
- [ ] Add iterator support to backend storage layer
- [ ] Implement `MainStorage::storage_iter_keys()` using backend iterator
- [ ] Add prefix filtering (only iterate Index/Entry keys, skip others)
- [ ] Add performance tests (iteration over 10K, 100K, 1M keys)
- [ ] Add batching/pagination for large datasets

**Files to modify:**
- `crates/storage/src/store.rs` - Replace placeholder implementation
- `crates/store/src/` - Add iterator support to storage backend

**Implementation hints:**
```rust
// For RocksDB backend
fn storage_iter_keys() -> Vec<Key> {
    let db = get_db_handle();
    let mut keys = Vec::new();
    
    let iter = db.iterator(IteratorMode::Start);
    for (key_bytes, _) in iter {
        // Deserialize key from bytes
        if let Some(key) = Key::from_bytes(&key_bytes) {
            keys.push(key);
        }
    }
    
    keys
}
```

**Acceptance Criteria:**
- [ ] `MainStorage::storage_iter_keys()` returns all keys
- [ ] GC works in production environment
- [ ] Snapshot generation works with real storage
- [ ] Performance is acceptable (< 1s for 10K keys)

---

### 1.2 Add Key Deserialization
**Status:** 🔴 Blocked
**Priority:** Critical
**Dependencies:** 1.1

**Tasks:**
- [ ] Add `Key::from_bytes([u8; 32])` method
- [ ] Handle all key variants (Index, Entry, SyncState)
- [ ] Add error handling for invalid keys
- [ ] Add tests for round-trip serialization

**Files to modify:**
- `crates/storage/src/store.rs`

**Implementation:**
```rust
impl Key {
    pub fn from_bytes(bytes: &[u8; 32]) -> Option<Self> {
        // Reverse the hash to get original bytes
        // This may require storing unhashed keys or using a key index
        // TODO: Design decision needed
        unimplemented!("Needs design decision on key storage")
    }
}
```

**Design Decision Required:**
- Option A: Store unhashed keys in separate index
- Option B: Use prefix iteration in backend (better)
- Option C: Maintain in-memory key set (memory overhead)

---

## Phase 2: Network Protocol Integration (High Priority)

### 2.1 Define Snapshot Transfer Protocol
**Status:** 🟡 Partial (Snapshot struct exists, needs network layer)
**Priority:** High
**Dependencies:** None
**Estimated Effort:** 3-5 days

**Tasks:**
- [ ] Define network message types:
  - `SNAPSHOT_REQUEST { node_id, reason }`
  - `SNAPSHOT_RESPONSE { snapshot, chunk_index, total_chunks }`
  - `SNAPSHOT_ACK { success, root_hash }`
- [ ] Add snapshot chunking for large datasets (> 10MB)
- [ ] Add compression (gzip or zstd)
- [ ] Add progress reporting
- [ ] Add timeout handling
- [ ] Add retry logic with exponential backoff

**Files to create/modify:**
- `crates/network/src/messages/snapshot.rs` - NEW
- `crates/network/src/handlers/snapshot.rs` - NEW
- `crates/node/src/sync/snapshot.rs` - NEW

**Protocol Design:**
```rust
// In network layer
pub enum SyncMessage {
    SnapshotRequest {
        requester_id: NodeId,
        reason: ResyncReason, // Manual, Timeout, etc.
    },
    SnapshotResponse {
        snapshot_chunk: Vec<u8>,
        chunk_index: u32,
        total_chunks: u32,
        compressed: bool,
    },
    SnapshotComplete {
        root_hash: [u8; 32],
        total_size: usize,
    },
}
```

**Acceptance Criteria:**
- [ ] Can request snapshot from remote node
- [ ] Snapshot transfers correctly (verify hash)
- [ ] Large snapshots (>100MB) transfer successfully
- [ ] Network failures handled gracefully
- [ ] Progress reporting works

---

### 2.2 Implement Snapshot Request Handler
**Status:** ⚪ Not Started
**Priority:** High
**Dependencies:** 2.1

**Tasks:**
- [ ] Add handler for `SNAPSHOT_REQUEST` messages
- [ ] Call `Interface::generate_snapshot()` when request received
- [ ] Chunk snapshot if large (> 10MB chunks)
- [ ] Send chunked response
- [ ] Add rate limiting (prevent snapshot DOS)
- [ ] Add authentication/authorization

**Files to modify:**
- `crates/node/src/sync/handlers.rs`
- `crates/server/src/handlers/sync.rs`

**Implementation:**
```rust
async fn handle_snapshot_request(
    requester_id: NodeId,
    reason: ResyncReason,
) -> Result<()> {
    // Check authorization
    if !is_authorized_for_snapshot(requester_id)? {
        return Err(SyncError::Unauthorized);
    }
    
    // Generate snapshot
    let snapshot = Interface::<MainStorage>::generate_snapshot()?;
    let snapshot_bytes = borsh::to_vec(&snapshot)?;
    
    // Chunk if large
    if snapshot_bytes.len() > 10_000_000 {
        send_chunked_snapshot(requester_id, snapshot_bytes).await?;
    } else {
        send_snapshot(requester_id, snapshot_bytes).await?;
    }
    
    Ok(())
}
```

---

### 2.3 Implement DeleteRef Action Propagation
**Status:** 🟡 Partial (Action exists, needs network propagation)
**Priority:** High
**Dependencies:** None

**Tasks:**
- [ ] Update sync action broadcast to include `DeleteRef`
- [ ] Ensure `Action::DeleteRef` is properly serialized over network
- [ ] Migrate existing code from `Delete` to `DeleteRef`
- [ ] Add backward compatibility for nodes using old `Delete` action

**Files to modify:**
- `crates/node/src/sync/propagate.rs`
- `crates/network/src/sync.rs`

**Implementation:**
```rust
// When entity is deleted locally
pub fn on_local_delete(parent_id: Id, collection: &str, child_id: Id) -> Result<()> {
    // Use new Interface method
    Interface::<MainStorage>::remove_child_from(parent_id, collection, child_id)?;
    
    // Propagate DeleteRef (not old Delete)
    let action = Action::DeleteRef {
        id: child_id,
        deleted_at: time_now(),
    };
    
    network::broadcast_action(action).await?;
    Ok(())
}
```

**Acceptance Criteria:**
- [ ] `DeleteRef` actions propagate to all peers
- [ ] Old nodes can still process actions (backward compat)
- [ ] Conflict resolution works across network
- [ ] Out-of-order messages handled correctly

---

## Phase 3: Garbage Collection Scheduling (Medium Priority)

### 3.1 Add GC Scheduler
**Status:** 🟡 Partial (GC function exists, needs scheduler)
**Priority:** Medium
**Dependencies:** 1.1 (storage iteration)
**Estimated Effort:** 2-3 days

**Tasks:**
- [ ] Create GC task/service in node
- [ ] Schedule GC to run every 12 hours (configurable)
- [ ] Add GC trigger on node startup (cleanup from previous run)
- [ ] Add manual GC trigger via API
- [ ] Add GC status monitoring
- [ ] Add graceful cancellation on shutdown

**Files to create/modify:**
- `crates/node/src/services/gc.rs` - NEW
- `crates/node/src/config.rs` - Add GC configuration
- `crates/server/src/admin/gc.rs` - Admin API

**Implementation:**
```rust
// In node service
pub struct GarbageCollector {
    interval: Duration,
    last_run: Instant,
    enabled: bool,
}

impl GarbageCollector {
    pub async fn run_periodic(&mut self) -> Result<()> {
        loop {
            tokio::time::sleep(self.interval).await;
            
            if !self.enabled {
                continue;
            }
            
            info!("Starting garbage collection...");
            let collected = Interface::<MainStorage>::garbage_collect_tombstones(
                TOMBSTONE_RETENTION_NANOS
            )?;
            info!("Garbage collection complete: {} tombstones removed", collected);
            
            self.last_run = Instant::now();
        }
    }
    
    pub fn trigger_manual_gc(&self) -> Result<usize> {
        Interface::<MainStorage>::garbage_collect_tombstones(TOMBSTONE_RETENTION_NANOS)
    }
}
```

**Acceptance Criteria:**
- [ ] GC runs automatically every 12 hours
- [ ] GC can be triggered manually
- [ ] GC metrics are logged
- [ ] GC doesn't block node operations
- [ ] GC configuration is respected

---

### 3.2 Add GC Metrics and Monitoring
**Status:** ⚪ Not Started
**Priority:** Medium
**Dependencies:** 3.1

**Tasks:**
- [ ] Add Prometheus metrics for GC:
  - `storage_gc_runs_total` (counter)
  - `storage_gc_tombstones_collected_total` (counter)
  - `storage_gc_duration_seconds` (histogram)
  - `storage_tombstones_current` (gauge)
- [ ] Add structured logging for GC events
- [ ] Add GC status to node health endpoint
- [ ] Add alerts for GC failures

**Files to modify:**
- `crates/node/src/services/gc.rs`
- `crates/node/src/metrics.rs`

**Implementation:**
```rust
pub struct GCMetrics {
    runs_total: Counter,
    tombstones_collected: Counter,
    duration: Histogram,
    tombstones_current: Gauge,
}

impl GarbageCollector {
    fn run_with_metrics(&self) -> Result<()> {
        let start = Instant::now();
        
        let collected = Interface::garbage_collect_tombstones(...)?;
        
        self.metrics.runs_total.inc();
        self.metrics.tombstones_collected.inc_by(collected as f64);
        self.metrics.duration.observe(start.elapsed().as_secs_f64());
        
        // Update current count (would need count_tombstones() method)
        // self.metrics.tombstones_current.set(count as f64);
        
        Ok(())
    }
}
```

---

## Phase 4: Sync Orchestration (Medium Priority)

### 4.1 Implement Sync Decision Logic
**Status:** 🟡 Partial (logic exists, needs integration)
**Priority:** Medium
**Dependencies:** 2.1, 2.2
**Estimated Effort:** 3-4 days

**Tasks:**
- [ ] Hook `sync_with_node()` into node's sync manager
- [ ] Track SyncState for each peer node
- [ ] Implement incremental sync path (compare_trees + apply_actions)
- [ ] Implement full resync fallback
- [ ] Add sync coordination lock (prevent concurrent syncs with same node)
- [ ] Add sync retry logic
- [ ] Log sync decisions and outcomes

**Files to modify:**
- `crates/node/src/sync/manager.rs`
- `crates/node/src/sync/orchestrator.rs` - NEW

**Implementation:**
```rust
pub struct SyncManager {
    active_syncs: HashMap<NodeId, SyncInProgress>,
    sync_states: HashMap<NodeId, SyncState>,
}

impl SyncManager {
    pub async fn sync_with_peer(&mut self, peer_id: NodeId) -> Result<()> {
        // Prevent concurrent syncs
        if self.active_syncs.contains_key(&peer_id) {
            return Err(SyncError::SyncInProgress);
        }
        
        // Mark sync as in progress
        self.active_syncs.insert(peer_id, SyncInProgress::new());
        
        // Get sync state
        let sync_state = Interface::get_sync_state(peer_id)?
            .unwrap_or_else(|| SyncState::new(peer_id));
        
        // Decide sync strategy
        let result = if sync_state.needs_full_resync(TOMBSTONE_RETENTION_NANOS) {
            info!("Full resync needed with peer {}", peer_id);
            self.perform_full_resync(peer_id).await
        } else {
            info!("Incremental sync with peer {}", peer_id);
            self.perform_incremental_sync(peer_id).await
        };
        
        // Clean up
        self.active_syncs.remove(&peer_id);
        
        result
    }
    
    async fn perform_full_resync(&self, peer_id: NodeId) -> Result<()> {
        // Request snapshot from peer
        let snapshot = network::request_snapshot(peer_id).await?;
        
        // Apply locally
        Interface::<MainStorage>::full_resync(peer_id, snapshot)?;
        
        info!("Full resync with {} completed successfully", peer_id);
        Ok(())
    }
    
    async fn perform_incremental_sync(&self, peer_id: NodeId) -> Result<()> {
        // Get comparison data from peer
        let foreign_comparison = network::request_comparison(peer_id).await?;
        
        // Compare trees
        let (local_actions, foreign_actions) = 
            Interface::<MainStorage>::compare_trees(foreign_comparison)?;
        
        // Apply foreign actions locally
        for action in foreign_actions {
            Interface::<MainStorage>::apply_action(action)?;
        }
        
        // Send local actions to foreign
        network::send_actions(peer_id, local_actions).await?;
        
        info!("Incremental sync with {} completed successfully", peer_id);
        Ok(())
    }
}
```

**Acceptance Criteria:**
- [ ] Node automatically chooses correct sync strategy
- [ ] SyncState is persisted and updated
- [ ] Concurrent syncs are prevented
- [ ] Sync failures are retried
- [ ] Metrics track sync operations

---

### 4.2 Handle Split-Brain Scenarios
**Status:** ⚪ Not Started
**Priority:** Medium
**Dependencies:** 4.1

**Tasks:**
- [ ] Detect split-brain (both nodes need full resync)
- [ ] Implement coordinator election or designated sync source
- [ ] Add conflict resolution strategy:
  - Option A: Prefer node with higher sync_count
  - Option B: Prefer node with more recent data (higher timestamps)
  - Option C: Designate always-online coordinator nodes
- [ ] Add manual override for admin to force sync direction
- [ ] Log split-brain events for monitoring

**Design Decision Required:**
Which strategy for split-brain resolution?

**Recommendation:** Use designated coordinator nodes (Option C) for production, with fallback to higher sync_count (Option A).

---

## Phase 5: Configuration Management (Medium Priority)

### 5.1 Add Storage Configuration to Node Config
**Status:** 🟢 Ready (constants exist, need config integration)
**Priority:** Medium
**Dependencies:** None
**Estimated Effort:** 1 day

**Tasks:**
- [ ] Add storage section to node config file
- [ ] Make retention periods configurable
- [ ] Make GC interval configurable
- [ ] Add config validation
- [ ] Add config documentation

**Files to modify:**
- `crates/node/src/config.rs`
- `crates/config/src/lib.rs`
- Node config examples

**Configuration structure:**
```toml
[storage]
# Tombstone retention period (how long deleted entities are kept for sync)
tombstone_retention = "1d"  # 1 day default

# Full resync threshold (when to force full resync)
full_resync_threshold = "2d"  # 2 days default

# Garbage collection interval
gc_interval = "12h"  # 12 hours default

# Enable automatic GC
gc_enabled = true

# GC batch size (max tombstones to collect per run, 0 = unlimited)
gc_batch_size = 10000

# Enable automatic full resync
auto_resync_enabled = true

# Max snapshot size before chunking (in bytes)
max_snapshot_size = 10485760  # 10MB
```

**Acceptance Criteria:**
- [ ] Config loads and validates correctly
- [ ] Default values match constants
- [ ] Invalid configs are rejected with clear errors
- [ ] Config changes require node restart (document this)

---

### 5.2 Add Configuration Validation
**Status:** ⚪ Not Started
**Priority:** Low
**Dependencies:** 5.1

**Tasks:**
- [ ] Validate `tombstone_retention < full_resync_threshold`
- [ ] Validate GC interval > 0
- [ ] Validate max snapshot size reasonable (1MB - 1GB)
- [ ] Add warnings for unusual configurations

---

## Phase 6: API Endpoints (Low-Medium Priority)

### 6.1 Add Admin GC Endpoint
**Status:** 🟢 Ready (GC function exists, needs API wrapper)
**Priority:** Medium
**Dependencies:** 3.1
**Estimated Effort:** 1 day

**Tasks:**
- [ ] Add POST `/admin/storage/gc` endpoint
- [ ] Require admin authentication
- [ ] Return GC statistics (count, duration)
- [ ] Add dry-run mode (preview what would be collected)

**Files to modify:**
- `crates/server/src/admin/storage.rs` - NEW

**API Design:**
```rust
// POST /admin/storage/gc
{
  "dry_run": false,
  "retention_override": "2d" // Optional
}

// Response
{
  "success": true,
  "tombstones_collected": 142,
  "duration_ms": 234,
  "storage_reclaimed_bytes": 7100
}
```

---

### 6.2 Add Sync Management Endpoints
**Status:** ⚪ Not Started
**Priority:** Low
**Dependencies:** 4.1

**Tasks:**
- [ ] Add GET `/admin/sync/state` - List sync states for all peers
- [ ] Add POST `/admin/sync/full-resync/{node_id}` - Trigger manual full resync
- [ ] Add GET `/admin/sync/status` - Current sync operations
- [ ] Add DELETE `/admin/sync/{node_id}` - Clear sync state (force resync)

**API Design:**
```rust
// GET /admin/sync/state
{
  "peers": [
    {
      "node_id": "abc123...",
      "last_sync": "2025-10-25T10:30:00Z",
      "sync_count": 142,
      "needs_full_resync": false,
      "offline_duration": "2h 15m"
    }
  ]
}
```

---

## Phase 7: Monitoring and Observability (Low-Medium Priority)

### 7.1 Add Storage Metrics
**Status:** ⚪ Not Started
**Priority:** Medium
**Dependencies:** None
**Estimated Effort:** 2 days

**Tasks:**
- [ ] Add Prometheus metrics:
  - `storage_tombstones_total` - Current tombstone count
  - `storage_tombstone_age_seconds` - Age distribution histogram
  - `storage_entities_total` - Total entities
  - `storage_sync_state_total` - Number of tracked peers
  - `storage_snapshot_generation_duration_seconds`
  - `storage_snapshot_size_bytes`
  - `storage_full_resync_total`
  - `storage_full_resync_duration_seconds`
- [ ] Add metrics collection points in code
- [ ] Add Grafana dashboard template

**Files to modify:**
- `crates/node/src/metrics.rs`
- `crates/context/src/metrics.rs`

---

### 7.2 Add Sync Event Logging
**Status:** ⚪ Not Started
**Priority:** Low
**Dependencies:** 4.1

**Tasks:**
- [ ] Log all sync decisions (incremental vs full)
- [ ] Log sync outcomes (success, failure, duration)
- [ ] Log GC runs and outcomes
- [ ] Add structured logging with context
- [ ] Add log levels appropriately (info for normal, warn for fallbacks)

---

## Phase 8: Testing and Validation (Critical)

### 8.1 Integration Tests
**Status:** ⚪ Not Started  
**Priority:** Critical
**Dependencies:** 1.1, 2.1, 4.1
**Estimated Effort:** 5-7 days

**Test Scenarios:**

**Storage Backend Tests:**
- [ ] Test: GC with 10K tombstones
- [ ] Test: GC with 100K tombstones
- [ ] Test: Snapshot generation with 1GB dataset
- [ ] Test: storage_iter_keys() performance

**Network Protocol Tests:**
- [ ] Test: Snapshot transfer (small <1MB)
- [ ] Test: Snapshot transfer (large >100MB)
- [ ] Test: Chunked snapshot transfer
- [ ] Test: Network failure during snapshot transfer
- [ ] Test: Snapshot corruption detection

**CRDT Tests:**
- [ ] Test: Delete vs Update conflict (delete older)
- [ ] Test: Delete vs Update conflict (delete newer)
- [ ] Test: Out-of-order DeleteRef messages
- [ ] Test: Duplicate DeleteRef messages
- [ ] Test: DeleteRef for non-existent entity

**Sync Orchestration Tests:**
- [ ] Test: Node offline 1 hour (incremental sync)
- [ ] Test: Node offline 1.5 days (incremental with fallback)
- [ ] Test: Node offline 3 days (full resync)
- [ ] Test: Split-brain scenario
- [ ] Test: Concurrent sync attempts
- [ ] Test: Sync during GC

**Full Resync Tests:**
- [ ] Test: Full resync with empty local storage
- [ ] Test: Full resync overwrites local data
- [ ] Test: Full resync with hash mismatch (should fail)
- [ ] Test: Full resync preserves SyncState
- [ ] Test: Multiple consecutive full resyncs

**Files to create:**
- `e2e-tests/src/steps/storage_gc.rs` - NEW
- `e2e-tests/src/steps/full_resync.rs` - NEW
- `e2e-tests/config/protocols/storage_sync.json` - NEW

---

### 8.2 Performance Tests
**Status:** ⚪ Not Started
**Priority:** Medium
**Dependencies:** 1.1, 2.1

**Tasks:**
- [ ] Benchmark GC with various tombstone counts
- [ ] Benchmark snapshot generation with various dataset sizes
- [ ] Benchmark full resync with various dataset sizes
- [ ] Measure memory usage during operations
- [ ] Identify bottlenecks and optimize

**Performance Targets:**
- GC: < 1s for 10K tombstones
- Snapshot gen: < 5s for 100MB dataset
- Full resync: < 60s for 100MB dataset
- Memory: < 100MB overhead during snapshot generation

---

## Phase 9: Documentation (Low Priority)

### 9.1 Update Node Documentation
**Status:** ⚪ Not Started
**Priority:** Low
**Dependencies:** All above

**Tasks:**
- [ ] Update node README with storage features
- [ ] Document GC behavior and configuration
- [ ] Document full resync triggers and process
- [ ] Add troubleshooting guide
- [ ] Add operational runbook

**Topics to cover:**
- How tombstone deletion works
- When full resync triggers
- How to manually trigger GC
- How to force full resync
- What to do if sync fails
- How to monitor storage health

---

### 9.2 Add Migration Guide
**Status:** ⚪ Not Started
**Priority:** Low

**Tasks:**
- [ ] Document migration from old deletion to tombstones
- [ ] Provide scripts to clean up orphaned data from old system
- [ ] Document backward compatibility approach
- [ ] Add rollback procedure if needed

---

## Phase 10: Production Deployment (Critical)

### 10.1 Add Feature Flags
**Status:** ⚪ Not Started
**Priority:** High
**Dependencies:** All phase 1-4

**Tasks:**
- [ ] Add feature flag for tombstone deletion (default: enabled)
- [ ] Add feature flag for auto-GC (default: enabled)
- [ ] Add feature flag for auto-full-resync (default: disabled initially)
- [ ] Add runtime toggle for features
- [ ] Add gradual rollout support

**Configuration:**
```toml
[storage.features]
tombstone_deletion_enabled = true
auto_gc_enabled = true
auto_full_resync_enabled = false  # Enable after validation
use_delete_ref_action = true  # vs legacy Delete
```

---

### 10.2 Migration and Rollout Plan
**Status:** ⚪ Not Started
**Priority:** Critical
**Dependencies:** 10.1

**Rollout Phases:**

**Phase 1: Enable Tombstones (Week 1)**
- [ ] Deploy code with tombstone support
- [ ] Enable `tombstone_deletion_enabled = true`
- [ ] Monitor for issues
- [ ] Verify DeleteRef actions work
- [ ] Keep auto-GC disabled

**Phase 2: Enable GC (Week 2)**
- [ ] Enable `auto_gc_enabled = true`
- [ ] Monitor GC runs
- [ ] Verify storage reclamation
- [ ] Check for performance impact

**Phase 3: Enable Full Resync (Week 3-4)**
- [ ] Enable `auto_full_resync_enabled = true`
- [ ] Test with controlled scenarios
- [ ] Monitor full resync occurrences
- [ ] Validate data consistency after resync

**Phase 4: Cleanup (Week 5+)**
- [ ] Migrate all Delete to DeleteRef
- [ ] Remove backward compatibility code (if desired)
- [ ] Optimize based on production metrics

---

## Quick Reference Checklist

### Must-Have for Production (Blocking)
- [ ] **1.1** - Backend storage iteration (enables GC)
- [ ] **2.1** - Snapshot transfer protocol (enables full resync)
- [ ] **2.3** - DeleteRef action propagation (enables CRDT)
- [ ] **4.1** - Sync orchestration (ties everything together)
- [ ] **8.1** - Integration tests (validates correctness)

### Should-Have for Production (Important)
- [ ] **3.1** - GC scheduler (automates cleanup)
- [ ] **5.1** - Configuration management (operability)
- [ ] **7.1** - Metrics (observability)
- [ ] **10.1** - Feature flags (safe rollout)

### Nice-to-Have (Can defer)
- [ ] **6.1-6.2** - Admin APIs (ops convenience)
- [ ] **7.2** - Enhanced logging (debugging)
- [ ] **8.2** - Performance tests (optimization)
- [ ] **9.1-9.2** - Documentation (user experience)

---

## Estimated Timeline

**Minimum Viable Integration (Must-Haves):**
- Backend iteration: 2-3 days
- Network protocol: 3-5 days
- Sync orchestration: 3-4 days
- Integration testing: 5-7 days
- **Total: 3-4 weeks**

**Production Ready (Must + Should Haves):**
- GC scheduler: 2-3 days
- Configuration: 1 day
- Metrics: 2 days
- Feature flags: 1 day
- **Total: 4-5 weeks**

**Full Implementation (Everything):**
- Admin APIs: 2-3 days
- Enhanced logging: 1 day
- Performance tests: 2-3 days
- Documentation: 2-3 days
- **Total: 6-7 weeks**

---

## Risk Assessment

### High Risks
1. **Storage iteration performance** - Could be slow with millions of keys
   - *Mitigation:* Use backend cursors, add batching
   
2. **Network bandwidth** - Large snapshots (>100MB) over slow connections
   - *Mitigation:* Compression, chunking, progress reporting
   
3. **Data loss during full resync** - If snapshot application fails
   - *Mitigation:* Backup before clear, rollback support
   
4. **Concurrent modifications during resync** - Data changes while syncing
   - *Mitigation:* Lock writes during resync, or use versioning

### Medium Risks
1. **GC removes needed tombstones** - If retention too short
   - *Mitigation:* Conservative defaults (1 day), configurable
   
2. **Split-brain loops** - Two nodes keep re-syncing
   - *Mitigation:* Coordinator election, sync locks

### Low Risks
1. **Config errors** - Invalid retention periods
   - *Mitigation:* Validation, sensible defaults

---

## Success Criteria

**Storage Integration Successful When:**
- ✅ No orphaned data accumulates
- ✅ Deletions sync correctly across nodes
- ✅ Delete vs Update conflicts resolve correctly
- ✅ Nodes offline 1-2 days resync successfully
- ✅ GC reclaims storage automatically
- ✅ Full resync works for extended offline periods
- ✅ Storage overhead < 100MB for typical workloads
- ✅ No performance degradation
- ✅ Zero data loss events

---

## Dependencies External to Storage

### Network Layer
- Snapshot request/response protocol
- Action broadcasting (DeleteRef)
- Chunked transfer support

### Node Layer
- Sync manager integration
- GC service/scheduler
- Configuration loading

### Store Backend
- Key iteration support
- Efficient prefix scanning

---

## Notes for Implementation

**When implementing, prioritize in this order:**
1. Backend iteration (unlocks GC)
2. Network protocol (unlocks full resync)
3. Sync orchestration (ties everything together)
4. Testing (validates correctness)
5. Monitoring (ensures production readiness)
6. Documentation (enables operations)

**Remember:**
- Test incrementally at each step
- Add feature flags for safe rollout
- Monitor metrics in staging before production
- Have rollback plan ready
- Document all design decisions

---

**Current Branch:** `perf/storage-optimization-and-docs`
**Status:** Storage layer complete ✅, integration work needed
**Next Step:** Review this roadmap, prioritize tasks, assign owners

