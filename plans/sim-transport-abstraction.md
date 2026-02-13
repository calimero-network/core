# Simulation Transport Abstraction Plan

## Branch
`feat/sim-transport-abstraction` (based on `origin/master` at `14c43567`)

## Goal
Enable the simulation framework to execute the **production HashComparison protocol code** through in-memory channels, so we can:
1. Test the actual production code path (not a reimplementation)
2. Verify invariants I4 (convergence) and I5 (CRDT merge, no overwrite)
3. Observe message flow and verify correctness

## Architecture: Simulation vs Production

The simulation uses **the exact same storage code path** as production:

```
┌─────────────────────────────────────────────────────────────────┐
│                     calimero-storage crate                       │
│  ┌─────────────────────────────────────────────────────────────┐│
│  │  Index<MainStorage>  │  Interface<MainStorage>              ││
│  │  (Merkle tree ops)   │  (Entity CRUD via apply_action)      ││
│  └──────────────────────┴──────────────────────────────────────┘│
│                              │                                   │
│                        RuntimeEnv                                │
│                    (read/write/remove callbacks)                 │
└─────────────────────────────────────────────────────────────────┘
                               │
                     calimero-store crate
                               │
              ┌────────────────┴────────────────┐
              │                                 │
     ┌────────┴────────┐              ┌────────┴────────┐
     │   Production    │              │   Simulation    │
     │  Store<RocksDB> │              │ Store<InMemoryDB>│
     └─────────────────┘              └─────────────────┘
```

**Same traits throughout:**
- `Database` trait - both RocksDB and InMemoryDB implement it
- `Index<MainStorage>` - identical usage  
- `Interface<MainStorage>` - identical usage
- `RuntimeEnv` callbacks - same signature

**The only difference is the `Database` implementation:**
- Production: `RocksDB` (persistent)
- Simulation: `InMemoryDB` (ephemeral)

This means Phase 2 (making protocol generic over `SyncTransport`) will allow testing 
the **actual production protocol code** with only the network layer swapped out.

---

## Current State

### What We Have
1. **`SyncTransport` trait** (`crates/node/primitives/src/sync/transport.rs`)
   - Abstracts network operations: `send`, `recv`, `recv_timeout`, `set_encryption`, `close`
   - Designed for both production and simulation use

2. **`StreamTransport`** (`crates/node/src/sync/stream.rs`)
   - Implements `SyncTransport` for production `calimero_network_primitives::stream::Stream`
   - NOT YET USED by production protocol code

3. **`SimStream`** (`crates/node/tests/sync_sim/transport.rs`)
   - Implements `SyncTransport` using `tokio::sync::mpsc` channels
   - In-memory bidirectional communication
   - Works correctly (unit tests pass)

4. **`protocol.rs`** (`crates/node/tests/sync_sim/protocol.rs`)
   - Contains `execute_hash_comparison_sync()` function
   - **PROBLEM**: This is a REIMPLEMENTATION of HashComparison logic, NOT the production code
   - Has `run_initiator`, `run_responder`, `build_tree_node_response` - all custom sim code

5. **`SimNode::new_in_context()`** (`crates/node/tests/sync_sim/node/state.rs`)
   - Creates nodes that share the same context ID
   - Correct model: nodes in same context sync together

6. **`SimStorage::update_entity_data()`** (`crates/node/tests/sync_sim/storage.rs`)
   - Allows updating/adding entities to the Merkle tree
   - Used by simulation protocol to apply received data

### Problems to Fix

#### Problem 1: Not Running Production Protocol
**Symptom**: `protocol.rs` reimplements HashComparison logic instead of calling production code.

**Why it matters**: 
- Bugs in simulation ≠ bugs in production
- Tests could pass while production is broken
- User explicitly asked: "why don't use production protocol directly?"

**Root cause**: Production `hash_comparison.rs` functions take concrete `Stream` type, not abstract `SyncTransport`.

**Solution**: Refactor production protocol to be generic over `SyncTransport`, then call it directly from simulation.

#### Problem 2: Entity Count Zero After Sync
**Symptom**: Test output shows:
```
entities_transferred: 4
Alice entity count after sync: 0
```

**Why it matters**: Data was "transferred" but Alice's entity count is 0. Root hashes match, but this is suspicious.

**Root cause**: `apply_leaf_to_storage` modifies the Merkle tree structure but may not be updating the entity tracking correctly. Need to investigate `SimStorage::update_entity_data` and how it interacts with `entity_count()`.

**Solution**: Verify `SimStorage` correctly tracks entities after updates. May need to update the entity map when `update_entity_data` is called.

#### Problem 3: CRDT Merge (I5) Not Properly Tested
**Symptom**: No test exists where both nodes have conflicting versions of the same entity.

**Why it matters**: Invariant I5 states "initialized nodes MUST CRDT-merge; overwrite ONLY for fresh nodes". We haven't verified this.

**Current behavior**: `apply_leaf_to_storage` uses simplified timestamp comparison:
```rust
// Simplified LWW - production would use full CRDT merge
if incoming_ts > local_ts {
    storage.update_entity_data(id, &leaf.data);
}
```

**Solution**: 
1. Add tests with conflicting entity versions
2. Verify CRDT merge semantics are applied correctly
3. Ensure newer timestamp wins (LWW) but merges don't lose data

---

## Implementation Plan

### Phase 1: Fix Entity Count Tracking ✅ COMPLETE
**Files**: `crates/node/tests/sync_sim/storage.rs`, `crates/node/tests/sync_sim/node/state.rs`

**Root Cause Found**:
- `SimNode::entity_count()` was using `self.entity_metadata.len()` - a separate cache
- When sync updates storage via `update_entity_data()`, the cache wasn't updated
- Storage clones share underlying DB (verified via Arc<dyn Database>), data was being written correctly

**Solution**:
- Added `SimStorage::leaf_count()` - counts only leaf nodes (actual entities, excludes intermediate nodes)
- Changed `SimNode::entity_count()` to use `storage.leaf_count()` instead of metadata cache
- This ensures sync results are visible while hierarchical structures are counted correctly

**Tasks Completed**:
- [x] 1.1: Root cause: metadata cache vs storage disconnect
- [x] 1.2: Added `leaf_count()` method to count only actual entities  
- [x] 1.3: Added tests verifying entity count after sync and async clone behavior

**Acceptance**: After sync, `alice.entity_count() == bob.entity_count()` ✅

### Phase 2: Refactor Production Protocol to Use SyncTransport ✅ COMPLETE
**Files**: 
- `crates/node/src/sync/hash_comparison.rs`
- `crates/node/src/sync/stream.rs`
- `crates/node/src/sync/manager.rs`

**Completed**:
- [x] 2.1: Identified functions using `Stream` directly
- [x] 2.2: Made `hash_comparison_sync<T: SyncTransport>` generic
- [x] 2.3: Made `handle_tree_node_request<T: SyncTransport>` generic
- [x] 2.4: Updated `StreamTransport` to use `&'a mut Stream` reference
- [x] 2.5: Updated call sites in `manager.rs` to use `StreamTransport::new(stream)`
- [x] 2.6: All 241 tests pass

**Key Changes**:
- `StreamTransport<'a>` now borrows `&'a mut Stream` (not owned)
- Protocol functions are generic over `T: SyncTransport`
- Production call sites wrap stream: `StreamTransport::new(stream)`

**Acceptance**: ✅ Production code compiles and all tests pass

### Phase 3: Replace Simulation Protocol with Production Code ✅ COMPLETE
**Files**:
- `crates/node/primitives/src/sync/protocol_trait.rs` (new)
- `crates/node/primitives/src/sync/storage_bridge.rs` (new)
- `crates/node/src/sync/hash_comparison_protocol.rs` (new)
- `crates/node/src/sync/hash_comparison.rs` (refactored to responder-only)
- `crates/node/tests/sync_sim/protocol.rs` (now calls production code)

**Architecture Implemented**:
- `SyncProtocolExecutor` trait in `node-primitives/src/sync/protocol_trait.rs`
- `HashComparisonProtocol` implements the trait
- `create_runtime_env` centralized in `storage_bridge.rs`
- Simulation calls production `HashComparisonProtocol::run_initiator/run_responder` directly

**Tasks Completed**:
- [x] 3.1: Defined `SyncProtocolExecutor` trait in `node-primitives/src/sync/`
- [x] 3.2: Made `create_runtime_env` reusable (shared helper in `storage_bridge.rs`)
- [x] 3.3: Implemented `SyncProtocolExecutor` for `HashComparisonProtocol`
- [x] 3.4: Updated `SyncManager` to call `HashComparisonProtocol::run_initiator`
- [x] 3.5: Updated simulation to call same `HashComparisonProtocol` functions
- [x] 3.6: Removed reimplemented protocol logic from `sync_sim/protocol.rs`

**Key Changes**:
- `wire.rs`: Changed `sequence_id` from `usize` to `u64` for portability (breaking change, documented)
- `hash_comparison.rs`: Refactored to responder-only (~875→285 lines), loop handles multi-request sessions
- `hash_comparison_protocol.rs`: Standalone initiator implementation using `Interface::apply_action`
- RuntimeEnv created once outside responder loop (optimization)

**Acceptance**: ✅
- Simulation calls the SAME protocol functions as production
- Only difference is `Store` backend (RocksDB vs InMemoryDB)
- Only difference is transport (Stream vs SimStream)

### Phase 4: Add CRDT Merge Tests (Invariant I5) ✅ COMPLETE
**Files**: `crates/node/tests/sync_sim/protocol.rs`, `crates/node/tests/sync_sim/scenarios/buffering.rs`

**Tasks Completed**:
- [x] 4.1: `test_three_node_crdt_conflict` - 3 nodes with conflicting entity values
- [x] 4.2: Verified merge via production `apply_leaf_with_crdt_merge` using `Interface::apply_action`
- [x] 4.3: Multiple 3-node scenarios: chain sync, mesh sync, fresh join
- [x] 4.4: Convergence verified (all nodes reach same root hash)

**Additional Tests Added**:
- `test_three_node_chain_sync` - A→B→C propagation
- `test_three_node_mesh_sync` - Full mesh convergence
- `test_three_node_fresh_join` - Empty node joins existing cluster
- `test_three_node_gossip_propagation` - Delta propagation via SimRuntime
- `test_gossip_delta_idempotent` - Duplicate delta delivery is safe

**Acceptance**: ✅ All CRDT merge scenarios pass, convergence verified

### Phase 5: Clean Up and Documentation ✅ COMPLETE
**Tasks Completed**:
- [x] 5.1: Debug prints removed (only meaningful test output remains)
- [x] 5.2: Old simulation protocol removed (~400 lines deleted from protocol.rs)
- [x] 5.3: Doc comments added to transport trait, protocol trait, storage_bridge
- [x] 5.4: PR review comments addressed (stale docs fixed, obsolete code removed)

**Code Hygiene**:
- `#[expect(dead_code)]` on `StreamTransport::with_timeout` (future use)
- `#[expect(clippy::type_complexity)]` on `StorageCallbacks` (acceptable complexity)
- Removed duplicate `generate_nonce()` function
- Removed redundant `SinkExt` import

---

## Key Invariants to Protect

From user rules:
- **I4**: Strategy equivalence (all strategies converge to same state)
- **I5**: No silent data loss (initialized nodes MUST CRDT-merge; overwrite ONLY for fresh nodes)
- **I6**: Deltas buffered + replayed via DAG during sync

## File Reference

| File | Purpose |
|------|---------|
| `crates/node/primitives/src/sync/transport.rs` | `SyncTransport` trait definition |
| `crates/node/src/sync/stream.rs` | `StreamTransport` for production |
| `crates/node/src/sync/hash_comparison.rs` | Production HashComparison protocol |
| `crates/node/tests/sync_sim/transport.rs` | `SimStream` for simulation |
| `crates/node/tests/sync_sim/protocol.rs` | Simulation protocol execution |
| `crates/node/tests/sync_sim/node/state.rs` | `SimNode` with context support |
| `crates/node/tests/sync_sim/storage.rs` | `SimStorage` Merkle tree |

## Success Criteria ✅ ALL MET

1. ✅ Simulation runs **production protocol code** through `SimStream`
2. ✅ Entity counts match after sync
3. ✅ CRDT merge tests pass (I5 verified)
4. ✅ All 247 tests pass (was 237, added 10 new tests)
5. ✅ No reimplemented protocol logic in simulation

---

## PR Status

**PR #1972**: `feat/sim-transport-abstraction`

All phases complete. Ready for final review and merge.

**Deferred items** (valid but out of scope):
- Encryption nonce rotation (encryption not yet used)
- DoS protection for responder loop (future hardening)
- Encryption unit tests (future work when encryption enabled)
