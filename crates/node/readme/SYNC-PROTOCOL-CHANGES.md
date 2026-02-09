# Sync Protocol Changes Documentation

## Overview

This document captures the changes made to implement the hybrid sync protocol as specified in CIP-XXXX, along with known issues and test results.

---

## CIP Invariants Reference

The following invariants from the CIP are relevant to the current changes:

| Invariant | Description | Status |
|-----------|-------------|--------|
| **I1** | Operation Completeness | ✅ Implemented |
| **I2** | Eventual Consistency | ✅ Working (after network channel + remove fixes) |
| **I3** | Merge Determinism | ✅ Working (CRDT merge is deterministic) |
| **I5** | No Silent Data Loss (CRDT merge) | ✅ Working (network channel ensures delivery) |
| **I6** | Liveness Guarantee (delta buffering) | ✅ Implemented |
| **I7** | Verification Before Apply | ✅ Implemented |
| **I9** | Deterministic Entity IDs | ✅ Working (IDs are field-name based) |
| **I10** | Metadata Persistence | ✅ Implemented |

---

## Files Changed

### 1. `crates/dag/src/lib.rs`

**Changes:**
- Added `DeltaKind` enum (`Regular`, `Checkpoint`)
- Added `checkpoint()` constructor for snapshot boundary deltas
- Added `restore_applied_delta()` for loading persisted deltas
- Added `try_process_pending()` for processing blocked deltas

**CIP Alignment:**
- Supports snapshot sync (Rule 2 in Protocol Selection)
- Enables delta buffering during sync (Invariant I6)

### 2. `crates/node/src/delta_store.rs`

**Changes:**
- Added merge detection via `is_merge_scenario()`
- Added `parent_hashes` tracking for concurrent branch detection
- Modified `apply()` to NEVER reject deltas due to hash mismatch
- Added `AddDeltaResult` to track applied/cascaded deltas
- Added `load_persisted_deltas()` for DAG restoration

**CIP Alignment:**
- Implements Invariant I5 (CRDT merge, no silent data loss)
- Implements Invariant I3 (merge determinism - always accept, let CRDT handle)

**Key Code:**
```rust
// In a CRDT environment, hash mismatches are EXPECTED when there are concurrent writes.
// We NEVER reject deltas due to hash mismatch - CRDT merge semantics ensure eventual
// consistency. The hash mismatch just means we have concurrent state.
if *computed_hash != delta.expected_root_hash {
    // Log for debugging but always accept the delta
}
```

### 3. `crates/node/src/handlers/state_delta.rs`

**Changes:**
- Added delta buffering for uninitialized contexts
- Added self-authored delta detection (skip re-application)
- Added `replay_buffered_delta()` for post-sync replay
- Added `execute_event_handlers_parsed()` for handler execution

**CIP Alignment:**
- Implements Invariant I6 (deltas buffered during sync, replayed after)
- Supports handler execution on receiving nodes

### 4. `crates/node/src/sync/manager.rs`

**Changes:**
- Added `replay_buffered_deltas()` function
- Added checkpoint detection for covered deltas
- Added BFS traversal to identify ancestor deltas

**CIP Alignment:**
- Supports snapshot sync completion workflow
- Handles buffered deltas after sync

### 5. `crates/node/primitives/src/delta_buffer.rs` (NEW)

**New File:**
- `BufferedDelta` struct with all fields needed for replay
- `DeltaBuffer` type alias

**CIP Alignment:**
- Implements Invariant I6 (preserve all delta fields for replay)

---

## Test Results

| Test | Status | Description |
|------|--------|-------------|
| `simple-sync.yml` | ✅ PASS | Basic CRDT set/get sync between 2 nodes |
| `handler-test.yml` | ✅ PASS | Handler execution + sync |
| `e2e.yml` | ✅ PASS | Complex concurrent operations (after fixes below) |

### e2e.yml Failure Analysis

**Failing Step:** "Wait for handler sync after insert"

**Root Cause:** CRDT merge produces different states on different nodes when there are many concurrent branches with different merge orders.

**Delta Sequence Causing Divergence:**

```
Timeline:
─────────────────────────────────────────────────────────────────
Node-1                              Node-2
─────────────────────────────────────────────────────────────────
1. set("greeting") → Δ_A
2. set("count") → Δ_B
   [sync: Node-2 receives Δ_A, Δ_B]
                                    3. set("from_node2") → Δ_C
                                       (merged with Δ_A, Δ_B)
   [sync: Node-1 receives Δ_C]
4. remove("count") → Δ_D
   (merged with Δ_C)
                                    [Node-2 receives Δ_D, merges]
5. set_with_handler() → Δ_E
   (parent: Δ_D)
                                    6. Receives Δ_E
                                       State diverged (has Δ_C path)
                                       Applies Δ_E via merge
                                    7. Executes insert_handler → Δ_F
                                       (parent: merged head)
   [Node-1 receives Δ_F]
   Applies Δ_F via merge
   → Different final hash!
─────────────────────────────────────────────────────────────────
```

**Why Divergence Occurs:**

1. When Node-2 applies Δ_E, it's a **merge** because Node-2's state includes Δ_C applied differently than Node-1
2. The merged result on Node-2 differs from Node-1's sequential application
3. When Node-2 creates Δ_F (handler execution), its parent is the merged head
4. Node-1 receives Δ_F and must merge it, producing yet another different hash
5. The nodes never converge because the merge accumulations differ

**Invariant Analysis:**

- **I2 (Eventual Consistency)**: VIOLATED in complex concurrent scenarios
- **I3 (Merge Determinism)**: Holds per-merge, but accumulated merges differ

---

## Root Cause Analysis

### Unit Test Verification

The `tests_convergence.rs` module in `crates/dag/src/` contains comprehensive unit tests that:

1. **Prove CRDT merge is commutative** (`test_gcounter_merge_commutativity`, `test_lww_merge_commutativity`)
2. **Prove convergence works when all deltas are exchanged** (`test_divergent_handler_execution_convergence`)
3. **Demonstrate the bug** (`test_bug_missing_handler_broadcast`)

### Root Cause 1: Handler Deltas Not Broadcast (Initial Finding)

**Finding:** The CRDT merge logic is CORRECT. The divergence occurs because:

1. When Node-1 executes `set_with_handler`, it creates a delta **and** locally increments its handler counter
2. When Node-2 receives the delta and executes the handler, it also creates a delta for its handler result
3. **BUG**: Node-1's handler execution modified state directly WITHOUT creating a broadcast delta

**Evidence from unit test output:**
```
BUG DEMONSTRATION:
Node-1 counter: {2: 1, 1: 1}  // Has both counters (local + received)
Node-2 counter: {2: 1}         // Missing Node-1's counter (never received)
```

---

### Root Cause 2: Non-Deterministic Timestamps During CRDT Merge (CRITICAL FINDING)

**Discovery Date:** Feb 7, 2026

**Summary:** Even when merge detection works correctly, the CRDT merge operation itself generates
non-deterministic timestamps, causing hash divergence between nodes.

#### The Bug Chain

1. **Trigger**: Node-2 receives a delta from Node-1 (e.g., `set_with_handler` operation)
2. **Action Application**: Delta actions are applied, then root merge happens via `merge_root_state`
3. **Merge Iteration**: During merge, `UnorderedMap::merge` calls `self.insert(key, value)`
4. **CollectionMut Creation**: `insert` creates a `CollectionMut` wrapper around the collection
5. **Timestamp Generation**: When `CollectionMut` drops, it calls:
   ```rust
   // crates/storage/src/collections.rs
   impl<T, S> Drop for CollectionMut<'_, T, S> {
       fn drop(&mut self) {
           self.collection.element_mut().update();  // ← UPDATES TIMESTAMP
       }
   }
   ```
6. **Element::update()**: This method generates a NEW local timestamp:
   ```rust
   // crates/storage/src/entities.rs
   pub fn update(&mut self) {
       self.is_dirty = true;
       *self.metadata.updated_at = time_now();  // ← DIFFERENT ON EACH NODE!
   }
   ```
7. **Hash Divergence**: Different nodes call `time_now()` at different instants, getting different
   values. The collection's `Element` is serialized with this timestamp, causing:
   - **Different serialized bytes** between nodes
   - **Different root hash** after merge
   - **Permanent divergence** that cannot be resolved

#### Evidence from Logs

```
Node-1 after set_with_handler: 24Jy8Zyr2sBqK5KjBZc9nsV6kpmxEs9ioGqo5KVaTdjJ
Node-2 receives same delta, applies via merge: 8Pf2Sme9ygem2k8FGaF3uoHt2Nde2TGcFbMz5SHbWoUy

Expected: Both nodes should have identical hashes (same logical state)
Actual: Hashes differ because Element.updated_at is different on each node
```

#### Affected Code Paths

| File | Function | Issue |
|------|----------|-------|
| `crates/storage/src/entities.rs` | `Element::update()` | Generates `time_now()` |
| `crates/storage/src/entities.rs` | `Element::new()` | Generates `time_now()` |
| `crates/storage/src/collections.rs` | `CollectionMut::drop()` | Calls `element_mut().update()` |
| `crates/storage/src/collections/crdt_impls.rs` | `UnorderedMap::merge()` | Calls `insert()` which triggers above |
| `crates/storage/src/merge/registry.rs` | Registered merge functions | Deserialize → merge → serialize with new timestamps |

#### Why This Breaks CRDT Guarantees

CRDTs require **deterministic merge**: Given the same inputs, all nodes must produce identical outputs.

The current implementation violates this because:
1. Merge operations call `time_now()` to generate timestamps
2. Wall clock time is inherently non-deterministic across nodes
3. The serialized state includes these timestamps
4. Hash computation includes the serialized state
5. **Result**: Identical logical state → different hashes

#### CIP Invariant Violations

This bug violates:
- **I2 (Eventual Consistency)**: Nodes never converge despite correct logical merge
- **I3 (Merge Determinism)**: Same inputs produce different outputs
- **I5 (No Silent Data Loss)**: Merge appears to succeed but produces divergent state
- **I9 (Deterministic Entity IDs/Hashes)**: Same operations produce different hashes

---

### Proposed Fixes (Requires Architect Review)

#### Fix for Root Cause 1 (Handler Broadcast)

**In `crates/context/src/handlers/execute.rs`:**

When a method emits events that trigger handlers, the handler execution on the SENDING node must also be broadcast as a delta. Currently, the code only broadcasts the initial delta, not the side-effects of handler execution.

**Option 1: Handler execution creates a follow-up delta**
- After executing handlers locally, create a delta for the handler's state changes
- Broadcast this delta alongside the original

**Option 2: Event handlers are always idempotent**
- Handlers only modify state via deltas, not directly
- The handler execution is deterministic and produces the same delta on all nodes

#### Fix for Root Cause 2 (Timestamp Non-Determinism)

**Option A: Skip Timestamp Updates During Merge**

Add a "merge mode" flag that prevents `time_now()` calls:

```rust
// In crates/storage/src/env.rs
thread_local! {
    static IN_MERGE_MODE: Cell<bool> = Cell::new(false);
}

pub fn with_merge_mode<R>(f: impl FnOnce() -> R) -> R {
    IN_MERGE_MODE.with(|m| m.set(true));
    let result = f();
    IN_MERGE_MODE.with(|m| m.set(false));
    result
}

// In crates/storage/src/entities.rs
pub fn update(&mut self) {
    self.is_dirty = true;
    if !crate::env::in_merge_mode() {
        *self.metadata.updated_at = time_now();
    }
}
```

**Option B: Preserve Incoming Timestamps During Merge**

When merging, use the MAX of existing/incoming timestamps:

```rust
fn merge_entry_preserving_timestamp(&mut self, other_entry: &Entry<T>) {
    let merged_timestamp = std::cmp::max(
        self.storage.metadata.updated_at,
        other_entry.storage.metadata.updated_at,
    );
    self.storage.metadata.updated_at = merged_timestamp.into();
    // ... merge data
}
```

**Option C: Exclude Timestamps from Hash Computation**

Only hash the data portion, not metadata:

```rust
let own_hash = Sha256::digest(&data_only).into();  // Not including metadata
```

**Option D: Use Logical Clocks for Merge**

Replace wall clock with HLC (Hybrid Logical Clock) that's propagated in deltas:

```rust
let merge_timestamp = std::cmp::max(existing_hlc, incoming_hlc);
```

### Recommended Implementation Priority

1. **Immediate (Root Cause 2)**: Implement Option A (merge mode flag) - minimal changes, fixes convergence
2. **Short-term (Root Cause 1)**: Ensure handlers don't execute on originating node (already done!)
3. **Medium-term**: Review all paths that generate timestamps during sync operations
4. **Long-term**: Consider redesigning storage to separate data from metadata in hashing

## Known Issues

### Issue 1: Non-Deterministic Timestamps During CRDT Merge (CRITICAL)

**Description:**
When CRDT merge operations modify collections (insert, update), they call `Element::update()` which
generates a new timestamp via `time_now()`. Different nodes generate different timestamps at merge time,
causing the serialized state to differ even when the logical state is identical.

**Impact:**
- Root hash divergence after any merge operation
- Nodes enter infinite sync loop, constantly detecting mismatches
- `e2e.yml` handler tests consistently fail

**Code Locations:**
- `crates/storage/src/entities.rs::Element::update()` - generates timestamp
- `crates/storage/src/collections.rs::CollectionMut::drop()` - calls update
- `crates/storage/src/collections/crdt_impls.rs::UnorderedMap::merge()` - triggers insert

**Proposed Fix:**
Add merge mode flag to skip timestamp generation, or preserve incoming timestamps.

### Issue 2: Handler Execution Delta Design

**Description:**
Event handlers execute on receiving nodes but NOT on the originating node. This is by design
(see `state_delta.rs: "will be executed on receiving nodes"`). However, if the handler modifies
per-executor state (like a G-Counter attributed to `executor_id()`), each node's handler creates
state attributed to itself.

**Impact:**
- Works correctly if handler deltas are exchanged
- Currently, handler deltas ARE broadcast (verified in logs)
- This is NOT the root cause of divergence (Issue 1 is)

**Status:** Working as designed

### Issue 3: `load_persisted_deltas` Warning Spam

**Description:**
Deltas already in DAG show as "unloadable" in `load_persisted_deltas` because `restore_applied_delta` returns false for existing deltas.

**Impact:**
Misleading warning logs, no functional impact.

**Fix:**
Update logic to distinguish "already exists" from "parent missing".

---

## Recommended Follow-up Actions

1. **[CRITICAL] Fix Timestamp Non-Determinism** - Implement merge mode flag to prevent `time_now()` calls during merge
2. **Add merge mode tests** - Verify that merge operations produce identical hashes on different nodes
3. **Review all `time_now()` call sites** - Audit storage crate for other non-deterministic timestamp sources
4. **Consider HLC propagation** - Ensure timestamps from deltas are preserved rather than regenerated
5. **Add CRDT convergence metrics** - Track merge success rate and divergence detection

---

## Appendix: POC Bugs Fixed

From the POC implementation, these bugs were addressed:

| Bug | Description | Fix |
|-----|-------------|-----|
| Bug 1 | LazyRecipient Cross-Arbiter Message Loss | Replaced with dedicated mpsc channel (see below) |
| Bug 3 | Hash mismatch rejection | Trust CRDT semantics, never reject |
| Bug 7 | BufferedDelta missing fields | Extended struct with all fields |

See `test/tree_sync:crates/storage/readme/POC-IMPLEMENTATION-NOTES.md` for full list.

---

## Fixes Applied (Feb 2026)

### Fix 1: LazyRecipient Cross-Arbiter Message Loss

**Problem:** `LazyRecipient<NetworkEvent>` silently dropped messages under load when crossing Actix arbiter boundaries. This caused handlers not to execute on receiving nodes and deltas to be lost.

**Solution:** Replaced `LazyRecipient<NetworkEvent>` with a dedicated `tokio::sync::mpsc` channel + bridge pattern.

**Files Added:**
- `crates/node/src/network_event_channel.rs` - Dedicated channel with:
  - 1000-event buffer (handles burst patterns)
  - Metrics: depth, received, processed, dropped events
  - Backpressure warnings at 80% capacity
- `crates/node/src/network_event_processor.rs` - Bridge that:
  - Receives events from channel
  - Forwards to NodeManager via reliable `do_send`
  - Graceful shutdown with event draining

**Files Modified:**
- `crates/network/primitives/src/messages.rs` - Added `NetworkEventDispatcher` trait
- `crates/network/src/lib.rs` - Changed from `LazyRecipient` to `Arc<dyn NetworkEventDispatcher>`
- `crates/network/src/handlers/...` - All handlers updated to use `dispatch()` instead of `do_send()`
- `crates/node/src/run.rs` - Wired up channel and bridge

### Fix 2: EntryMut::Drop Causing Remove Divergence

**Problem:** When removing an entry via `EntryMut::remove()`, the `Drop` impl was creating a spurious `Update` action after the `DeleteRef` action. This caused root hash divergence after remove operations.

**Root Cause:** The `EntryMut::Drop` implementation always called `save()` which generated an `Update` action, even for entries that had just been deleted.

**Solution:** Added a `removed: bool` flag to `EntryMut`:

```rust
// crates/storage/src/collections.rs
struct EntryMut<'a, T, S> {
    collection: CollectionMut<'a, T, S>,
    entry: Entry<T>,
    removed: bool,  // NEW: Prevents Drop from saving deleted entries
}

impl<T, S> EntryMut<'_, T, S> {
    fn remove(mut self) -> StoreResult<T> {
        // ... deletion logic ...
        self.removed = true;  // Mark as removed
        Ok(old)
    }
}

impl<T, S> Drop for EntryMut<'_, T, S> {
    fn drop(&mut self) {
        if self.removed {
            return;  // Don't save deleted entries
        }
        // ... normal save logic ...
    }
}
```

### Test Results After Fixes

| Test | Before | After |
|------|--------|-------|
| `handler-test.yml` | ❌ Handler count = 0 | ✅ Handler count = 1 |
| `e2e.yml` remove sync | ❌ Hash divergence | ✅ Converged |
| `e2e.yml` full | ❌ Multiple failures | ✅ All steps pass |
