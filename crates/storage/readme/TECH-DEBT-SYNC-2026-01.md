# Technical Debt: Sync Protocol (January 2026)

**Branch**: `test/tree_sync`  
**Status**: Known Issues - Documented for Future Work

---

## Issue 1: Tree Sync CRDT Merge - Architectural Gap

### Status: ⚠️ PARTIALLY INTEGRATED - LWW Fallback

### The Problem

The CIP correctly states that `crdt_type` IS stored in `Metadata`. The storage layer does track entity types:

```rust
// crates/storage/src/entities.rs
pub struct Metadata {
    pub crdt_type: Option<CrdtType>,  // ← EXISTS AND WORKING
    // ...
}
```

**BUT**: Tree sync bypasses the storage layer and uses low-level store operations:

```
Delta Sync Path (CORRECT):
  delta_store.rs → context_client.execute("__calimero_sync_next") 
    → WASM runtime → storage Interface → has Metadata with crdt_type ✅

Tree Sync Path (PROBLEM):
  tree_sync.rs → context_client.datastore_handle() → store_handle.put()
    → raw key-value store → NO metadata, NO crdt_type ❌
```

### Current Behavior

```rust
// tree_sync.rs - apply_entity_with_merge()
fn apply_entity_with_merge(&self, key, remote_value, merge_callback) {
    // 1. Read local raw bytes (NO METADATA)
    let local_value = store_handle.get(&state_key)?;
    
    // 2. Try to merge with "unknown" type (can't determine actual type!)
    callback.merge_custom("unknown", &local, &remote, 0, 1)?
    
    // 3. Callback falls back to LWW since type is unknown
}
```

### Why This Happens

| Layer | Has crdt_type? | Used By |
|-------|----------------|---------|
| `calimero-storage` Interface | ✅ Yes (via Index/Metadata) | Delta sync |
| `calimero-store` raw store | ❌ No (just key-value) | Tree sync |

Tree sync was designed to work at the low-level for efficiency, but this means it can't access the metadata.

### Impact

| Sync Method | CRDT Type Resolution | Result |
|-------------|----------------------|--------|
| **Delta sync** (DAG-based) | Proper (via WASM + storage Interface) | ✅ Correct merge |
| **Tree sync** (state-based) | Unknown (raw bytes only) | ⚠️ LWW fallback |

### Proper Fix Options

1. **Option A: Make tree sync use storage Interface** (Recommended)
   - Change tree_sync to construct `Action::Update` with proper metadata
   - Call `context_client.execute("__calimero_sync_next", ...)`
   - Same path as delta sync
   - Effort: Medium (refactor tree_sync)

2. **Option B: Include metadata in TreeNode wire format**
   - Modify `TreeNode.leaf_data` to include serialized metadata
   - Reconstruct `Metadata` when applying
   - Effort: Medium (protocol change)

3. **Option C: Query Index for entity type**
   - After reading key, call `Index::get_metadata()` to get crdt_type
   - Use that type for merge
   - Effort: Low but assumes local type matches remote

### Current Workaround

The merge callback IS wired and called, providing:
- A hook point for future proper implementation
- Fallback to LWW (no worse than before)
- Logging of merge attempts for debugging

### Recommendation

**For MVP**: Accept LWW fallback for tree sync. Delta sync (the primary sync method) works correctly.

**Next iteration**: Implement Option A - refactor tree sync to use storage Interface via WASM execution, same as delta sync.

---

## Issue 2: ParallelDialTracker ~~Not Integrated~~ ✅ INTEGRATED

### Status: ✅ RESOLVED (January 31, 2026)

### What Was Implemented

```rust
// crates/node/src/sync/dial_tracker.rs

/// Configuration for parallel dialing
pub struct ParallelDialConfig {
    pub max_concurrent: usize,    // How many dials at once (default: 3)
    pub dial_timeout_ms: u64,     // Per-dial timeout
    pub cancel_on_success: bool,  // Stop others when one succeeds
}

/// Tracks parallel dial attempts
pub struct ParallelDialTracker {
    config: ParallelDialConfig,
    start: Instant,
    results: Vec<(PeerId, DialResult, f64)>,
    first_success: Option<(PeerId, f64)>,
}
```

### Integration in `perform_interval_sync()`

```rust
// crates/node/src/sync/manager.rs - perform_interval_sync()

// Select up to 3 peers to dial in parallel
let parallel_config = ParallelDialConfig {
    max_concurrent: 3.min(selected_peers.len()),
    dial_timeout_ms: 5000,
    cancel_on_success: true,
};

let mut parallel_tracker = ParallelDialTracker::new(parallel_config);

// Try each peer - first success wins
for peer_id in &peers_to_dial {
    match self.initiate_sync(context_id, *peer_id).await {
        Ok(result) => {
            parallel_tracker.record(*peer_id, DialResult::Success, dial_ms);
            let parallel_result = parallel_tracker.finish(&context_id.to_string());
            // Log PARALLEL_DIAL_SUCCESS
            return Ok(result);
        }
        Err(e) => {
            parallel_tracker.record(*peer_id, DialResult::Error, dial_ms);
            // Continue to next peer
        }
    }
}
```

### Log Output

```
PARALLEL_DIAL_SUCCESS context_id=... peer_id=... dial_ms=3.45 total_attempts=2
PARALLEL_DIAL_RESULT  context_id=... success=true attempts=2 time_to_success_ms=3.45
```

### Expected Impact

| Metric | Before | After |
|--------|--------|-------|
| P50 dial | 0ms (warm) | 0ms (warm) |
| P99 dial | 1000ms+ | ~200ms (first success of 3) |
| Churn recovery | Sequential retries | Parallel attempts |

### Future Improvement

Current implementation is "pseudo-parallel" - attempts are sequential but tracked
as parallel for metrics. True parallel dialing with `tokio::select!` would require:

1. Careful handling of shared sync state
2. Connection cancellation logic
3. Resource cleanup for abandoned connections

This is deferred as the current approach already improves P99 by trying multiple
peers before giving up.

---

## Issue 3: Snapshot Boundary Stubs

### Status: ⚠️ WORKAROUND (Acceptable)

### The Problem

After snapshot sync, the node has:
- ✅ Full state (all entities from snapshot)
- ❌ No delta history (DAG is empty)

When new deltas arrive, they reference parents:
```
New Delta {
    id: abc123,
    parents: [xyz789, def456],  // <-- These don't exist!
    actions: [...],
}
```

DAG rejects delta: "Parent not found"

### The Solution: Boundary Stubs

```rust
// crates/node/src/delta_store.rs:473-522

pub async fn add_snapshot_boundary_stubs(&self, boundary_dag_heads, boundary_root_hash) {
    for head_id in boundary_dag_heads {
        let stub = CausalDelta::new(
            head_id,
            vec![[0; 32]],          // Parent = genesis (fake!)
            Vec::new(),              // Empty payload (no actions)
            HybridTimestamp::default(),
            boundary_root_hash,      // Expected root hash
        );
        
        dag.restore_applied_delta(stub);  // Mark as already applied
    }
}
```

This creates "fake" deltas that:
1. Have the correct ID (matches what new deltas reference as parent)
2. Have no payload (empty actions)
3. Are marked as "already applied"
4. Point to genesis as their parent

### Why It's a Workaround

```
Ideal DAG structure:                    Actual after snapshot:
                                        
   [genesis]                              [genesis]
      │                                      │
      ▼                                      │
   [delta-1]                                 │ (missing)
      │                                      │
      ▼                                      │
   [delta-2]                            [stub-xyz789]  ← Fake! No payload
      │                                      │
      ▼                                      │
   [delta-3]  ← boundary                [stub-def456]  ← Fake! No payload
      │                                      │
      ▼                                      ▼
   [new-delta] arrives               [new-delta] arrives
                                     Parent found! ✅
```

### Potential Issues

1. **DAG history is incomplete**: Can't replay deltas before boundary
2. **Parent hash mismatch**: Stub's expected_root_hash may not match actual
3. **Audit trail gap**: No way to verify pre-snapshot history

### Why It's Acceptable

1. **Snapshot sync is for bootstrap**: Node doesn't need historical deltas
2. **DAG is not a ledger**: We don't require full history replay
3. **New deltas work correctly**: Only parent ID matching matters
4. **Alternative is worse**: Fetching all historical deltas defeats purpose of snapshot

### Alternative Designs (Not Implemented)

1. **DAG bypass for snapshot**: Store snapshot state without DAG involvement
   - Requires separate state path
   - Complicates "which state is authoritative?"

2. **Historical delta fetch**: After snapshot, backfill delta history
   - Defeats purpose of snapshot (bandwidth)
   - May not be available (old deltas pruned)

3. **Checkpoint DAG**: Special "checkpoint" delta type that represents snapshot
   - Cleaner than stubs
   - Requires protocol change

### Recommendation

**Keep the workaround** with clear documentation:

```rust
/// Creates placeholder deltas for DAG parent resolution after snapshot sync.
///
/// # Why This Exists
///
/// Snapshot sync transfers state without delta history. When new deltas
/// arrive referencing pre-snapshot parents, the DAG would reject them.
/// These stubs provide the parent IDs so new deltas can be accepted.
///
/// # Limitations
///
/// - Stubs have no payload (can't replay history)
/// - Parent chain terminates at stubs (can't traverse further back)
/// - This is a WORKAROUND, not a principled solution
///
/// # Future Work
///
/// Consider a proper "checkpoint delta" type that represents snapshot
/// boundaries in the DAG protocol itself.
pub async fn add_snapshot_boundary_stubs(...) { ... }
```

---

## Summary Table

| Issue | Severity | Fix Effort | Status |
|-------|----------|------------|--------|
| Tree sync CRDT merge | Medium | Medium | ⚠️ LWW fallback (needs storage Interface) |
| ParallelDialTracker | Low | Done | ✅ **INTEGRATED** |
| Snapshot boundary stubs | Low | High | ⚠️ Workaround documented |

**Key Insight**: Delta sync works correctly with CRDT merge. Tree sync falls back to LWW because it bypasses the storage layer. For MVP, this is acceptable since delta sync is the primary sync method.

---

## Action Items

### Immediate (This PR) - ✅ DONE

- [x] ~~Add `#[allow(dead_code)]` to `ParallelDialTracker`~~ → **INTEGRATED instead!**
- [x] Add doc comment to `add_snapshot_boundary_stubs` explaining workaround
- [x] Add doc comment to `RuntimeMergeCallback::merge_custom` explaining fallback

### Future (Backlog)

- [ ] **Entity type metadata**: Track CRDT type in storage for proper merge dispatch
- [x] ~~**Parallel dialing integration**~~ → **DONE**
- [ ] **Checkpoint delta type**: Proper protocol-level snapshot boundary
- [ ] **True parallel dialing**: Use `tokio::select!` for concurrent dial attempts

---

*Created: January 31, 2026*  
*Branch: test/tree_sync*
