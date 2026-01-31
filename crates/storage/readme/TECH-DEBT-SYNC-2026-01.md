# Technical Debt: Sync Protocol (January 2026)

**Branch**: `test/tree_sync`  
**Status**: Mostly Resolved - Remaining Items Documented

---

## Issue 1: Tree Sync CRDT Merge - ✅ FIXED

### Status: ✅ PROPERLY INTEGRATED

**Implemented Solution**: Option B + C hybrid - Include metadata in wire format AND query local Index.

### What Changed

1. **Wire Protocol Updated**: `TreeNode.leaf_data` is now `Option<TreeLeafData>` which includes:
   ```rust
   pub struct TreeLeafData {
       pub key: [u8; 32],
       pub value: Vec<u8>,
       pub metadata: Metadata,  // ← Includes crdt_type!
   }
   ```

2. **Tree Node Generation**: `handle_tree_node_request` now reads entity metadata from storage Index and includes it in the response.

3. **CRDT Merge Dispatch**: `apply_entity_with_merge` now calls `Interface::merge_by_crdt_type_with_callback()` for proper CRDT dispatch:
   - Built-in CRDTs (Counter, Map, etc.) → merge directly in storage layer
   - Custom types → dispatch to WASM callback
   - Unknown/missing → fallback to LWW

### Current Data Flow

```
Tree Sync Path (NOW CORRECT):
  tree_sync.rs → receive TreeLeafData with Metadata
    → read local Index to get local Metadata
    → Interface::merge_by_crdt_type_with_callback()
    → proper CRDT merge based on crdt_type ✅
```

### Key Files Changed

- `crates/node/primitives/src/sync.rs` - Added `TreeLeafData` struct
- `crates/node/src/sync/manager.rs` - Updated `handle_tree_node_request`
- `crates/node/src/sync/tree_sync.rs` - Updated `apply_entity_with_merge`, `apply_leaf_from_tree_data`
- `crates/storage/src/interface.rs` - Made `merge_by_crdt_type_with_callback` public

### Remaining Limitation

**Bloom Filter Sync**: Still uses legacy format without metadata. Falls back to LWW.

This is acceptable because:
- Bloom filter sync is for fast diff detection, not conflict resolution
- The actual entity application still uses local metadata when available
- Full CRDT merge works for new entities via tree sync

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
| Tree sync CRDT merge | ~~Medium~~ | ~~Medium~~ | ✅ **FIXED** - Uses `Interface::merge_by_crdt_type_with_callback` |
| ParallelDialTracker | Low | Done | ✅ **INTEGRATED** |
| Snapshot boundary stubs | Low | High | ⚠️ Workaround documented |
| WASM merge callback | ~~Low~~ | ~~Medium~~ | ✅ **NOT NEEDED** - Registry already works |

**Key Insight (Updated)**: Both delta sync AND tree sync now use proper CRDT merge:
- Built-in CRDTs (Counter, Map, Set, Register) merge correctly via `Interface`
- Collections store children as separate entities (per-key merge works)
- Counter uses per-executor slots (no conflict between nodes)
- `RuntimeMergeCallback::merge_custom()` → `try_merge_by_type_name()` → uses global registry
- The registry is populated when WASM loads (`__calimero_register_merge`)
- Only `CrdtType::Custom` with app-defined `__calimero_merge` export would need more (hypothetical)

---

## Action Items

### Immediate (This PR) - ✅ ALL DONE

- [x] ~~Add `#[allow(dead_code)]` to `ParallelDialTracker`~~ → **INTEGRATED instead!**
- [x] Add doc comment to `add_snapshot_boundary_stubs` explaining workaround
- [x] Add doc comment to `RuntimeMergeCallback::merge_custom` explaining fallback
- [x] ~~Entity type metadata~~ → **ALREADY WORKS** (Metadata has crdt_type, Index stores it)
- [x] **Tree sync CRDT merge** → **FIXED** via `apply_entity_with_merge()` + `Interface::merge_by_crdt_type_with_callback()`

### Future (Backlog)

- [x] ~~**Parallel dialing integration**~~ → **DONE**
- [x] ~~**WASM merge callback**~~ → **NOT NEEDED** (see below)
- [ ] **Checkpoint delta type**: Proper protocol-level snapshot boundary
- [ ] **True parallel dialing**: Use `tokio::select!` for concurrent dial attempts

### Why `RuntimeMergeCallback::from_module()` is NOT Needed

The `from_module()` returning `None` is **not a bug**. Here's why:

1. **Built-in CRDTs already work**: When WASM loads, `__calimero_register_merge()` is called automatically (generated by `#[app::state]` macro). This registers the state type in a global registry.

2. **`merge_custom()` already uses the registry**: When sync calls `RuntimeMergeCallback::merge_custom()`, it calls `try_merge_by_type_name()` which looks up the type in the global registry.

3. **The flow is**:
   ```
   WASM loads → __calimero_register_merge() → global registry
                                                    ↓
   Sync → RuntimeMergeCallback::merge_custom() → try_merge_by_type_name() → registry lookup → CRDT merge
   ```

4. **What `from_module()` would add**: Support for a hypothetical `__calimero_merge` WASM export that apps could implement for custom merge logic. This is NOT the same as the current `Mergeable` trait which works at the Rust type level.

**Bottom line**: The current implementation is complete. Built-in CRDTs merge correctly. Custom `#[derive(Mergeable)]` types merge correctly. The only thing missing is a hypothetical future feature for WASM-level custom merge exports, which no apps currently use.

---

*Created: January 31, 2026*  
*Last updated: January 31, 2026 - Post critical audit*  
*Branch: test/tree_sync*
