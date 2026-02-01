# Technical Debt: Sync Protocol (January 2026)

**Branch**: `test/tree_sync`  
**Status**: ✅ CODE COMPLETE

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

### All Sync Strategies Complete

All tree sync strategies now use `TreeLeafData` with metadata:
- ✅ HashComparison
- ✅ BloomFilter (fixed in aa70ee48)
- ✅ SubtreePrefetch  
- ✅ LevelWise

---

## Issue 2: ParallelDialTracker - ✅ TRUE PARALLEL DIALING

### Status: ✅ COMPLETE (February 1, 2026)

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

### Implementation

**TRUE parallel dialing using `FuturesUnordered`**:

```rust
// Create concurrent dial futures
let mut dial_futures: FuturesUnordered<_> = peers_to_dial
    .iter()
    .map(|&peer_id| async move {
        let result = self.initiate_sync(context_id, peer_id).await;
        (peer_id, result, dial_ms)
    })
    .collect();

// Race all - first success wins, others are dropped
while let Some((peer_id, result, dial_ms)) = dial_futures.next().await {
    if result.is_ok() {
        drop(dial_futures); // Cancel remaining
        return Ok(result);
    }
}
```

Benefits:
- All dial attempts run truly concurrently
- First success immediately returns
- Remaining futures are cancelled (dropped)
- No sequential blocking

---

## Issue 3: Snapshot Boundary - ✅ PROPER CHECKPOINT DELTAS

### Status: ✅ FIXED (February 1, 2026)

### The Problem

After snapshot sync, the node has:
- ✅ Full state (all entities from snapshot)
- ❌ No delta history (DAG is empty)

When new deltas arrive, they reference parents that don't exist → DAG rejects them.

### The Solution: Checkpoint Deltas

**Proper protocol-level fix**: Added `DeltaKind` enum to `CausalDelta`:

```rust
// crates/dag/src/lib.rs

pub enum DeltaKind {
    /// Regular delta with operations to apply
    Regular,
    /// Checkpoint delta representing a snapshot boundary
    Checkpoint,
}

pub struct CausalDelta<T> {
    pub id: [u8; 32],
    pub parents: Vec<[u8; 32]>,
    pub payload: T,
    pub hlc: HybridTimestamp,
    pub expected_root_hash: [u8; 32],
    pub kind: DeltaKind,  // NEW!
}

impl<T> CausalDelta<T> {
    /// Create a checkpoint delta for snapshot boundary
    pub fn checkpoint(id: [u8; 32], expected_root_hash: [u8; 32]) -> Self
    where T: Default {
        Self {
            id,
            parents: vec![[0; 32]],  // Genesis parent
            payload: T::default(),   // Empty payload
            hlc: HybridTimestamp::default(),
            expected_root_hash,
            kind: DeltaKind::Checkpoint,
        }
    }
}
```

### Usage

```rust
// crates/node/src/delta_store.rs

pub async fn add_snapshot_checkpoints(
    &self,
    boundary_dag_heads: Vec<[u8; 32]>,
    boundary_root_hash: [u8; 32],
) -> usize {
    for head_id in boundary_dag_heads {
        let checkpoint = CausalDelta::checkpoint(head_id, boundary_root_hash);
        dag.restore_applied_delta(checkpoint);
    }
}
```

### Benefits

1. **Protocol-level**: Checkpoints are first-class DAG citizens
2. **Self-documenting**: `kind: Checkpoint` vs `kind: Regular`
3. **Backward compatible**: `#[serde(default)]` handles old deltas
4. **Proper API**: `CausalDelta::checkpoint()` vs struct literal hack

---

## Summary Table

| Issue | Status |
|-------|--------|
| Tree sync CRDT merge | ✅ FIXED |
| Bloom filter metadata | ✅ FIXED |
| True parallel dialing | ✅ DONE |
| WASM merge callback | ✅ NOT NEEDED |
| Snapshot checkpoints | ✅ FIXED (DeltaKind::Checkpoint) |

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
- [x] ~~Add doc comment to `add_snapshot_boundary_stubs`~~ → **REPLACED with `add_snapshot_checkpoints`**
- [x] Add doc comment to `RuntimeMergeCallback::merge_custom` explaining fallback
- [x] ~~Entity type metadata~~ → **ALREADY WORKS** (Metadata has crdt_type, Index stores it)
- [x] **Tree sync CRDT merge** → **FIXED** via `apply_entity_with_merge()` + `Interface::merge_by_crdt_type_with_callback()`

### Future (Backlog)

- [x] ~~**Parallel dialing integration**~~ → **DONE**
- [x] ~~**WASM merge callback**~~ → **NOT NEEDED** (see below)
- [x] ~~**True parallel dialing**~~ → **DONE** (uses `FuturesUnordered`)
- [x] ~~**Checkpoint delta type**~~ → **DONE** (`DeltaKind::Checkpoint`)

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
*Last updated: February 1, 2026 - CODE COMPLETE*  
*Branch: test/tree_sync*
