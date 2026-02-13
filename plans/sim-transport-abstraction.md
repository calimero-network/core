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

### Phase 3: Replace Simulation Protocol with Production Code
**Files**:
- `crates/node/src/sync/hash_comparison.rs`
- `crates/node/tests/sync_sim/protocol.rs`

**Key Insight**: No new trait needed! All dependencies route back to `Store`:

| SyncManager Dependency | What it actually uses |
|------------------------|----------------------|
| `context_client.datastore_handle()` | `Store` handle |
| `context_client.get_context()` | Context metadata (root_hash) |
| `context_client.get_context_members()` | Identity lookup |
| `get_local_tree_node_from_index()` | `Index<MainStorage>` via RuntimeEnv |
| `apply_leaf_with_crdt_merge()` | Store write + merge |
| `merge_entity_values()` | `calimero_storage::merge::merge_root_state` |

**Simulation already has all of this:**
- `SimStorage` wraps `Store<InMemoryDB>` ✅
- `SimNode` has `context_id()` and `root_hash()` ✅
- Can use fixed test identity ✅
- `Index<MainStorage>` works via `RuntimeEnv` ✅

**Approach**: Extract to standalone functions (no new traits):

```rust
// Extracted standalone function
pub async fn hash_comparison_initiator<T: SyncTransport>(
    transport: &mut T,
    store: &Store,           // Works with RocksDB or InMemoryDB!
    context_id: ContextId,
    our_identity: PublicKey,
    remote_root_hash: [u8; 32],
) -> Result<HashComparisonStats>

pub async fn hash_comparison_responder<T: SyncTransport>(
    transport: &mut T,
    store: &Store,
    context_id: ContextId,
    our_identity: PublicKey,
) -> Result<()>
```

**Architecture**: Define a common `SyncProtocol` trait that all protocols implement:

```rust
#[async_trait]
pub trait SyncProtocol {
    type Config;  // Protocol-specific params
    type Stats;   // Protocol-specific results
    
    async fn run_initiator<T: SyncTransport>(
        transport: &mut T,
        store: &Store,
        context_id: ContextId,
        identity: PublicKey,
        config: Self::Config,
    ) -> Result<Self::Stats>;
    
    async fn run_responder<T: SyncTransport>(
        transport: &mut T,
        store: &Store,
        context_id: ContextId,
        identity: PublicKey,
    ) -> Result<()>;
}
```

**Tasks**:
- [ ] 3.1: Define `SyncProtocol` trait in `node-primitives/src/sync/`
- [ ] 3.2: Make `create_runtime_env` reusable (shared helper for all protocols)
- [ ] 3.3: Implement `SyncProtocol` for `HashComparisonProtocol`
- [ ] 3.4: Update `SyncManager` to call `HashComparisonProtocol::run_initiator/responder`
- [ ] 3.5: Update simulation to call same `HashComparisonProtocol` functions
- [ ] 3.6: Remove reimplemented protocol logic from `sync_sim/protocol.rs`

**Out of scope**: Protocol negotiation refactor (future work)

**Technical Debt Note**: `RuntimeEnv::new()` duplicated in 4 places - Phase 3 consolidates sync-related ones

**Acceptance**: 
- Simulation calls the SAME protocol functions as production
- Only difference is `Store` backend (RocksDB vs InMemoryDB)
- Only difference is transport (Stream vs SimStream)

### Phase 4: Add CRDT Merge Tests (Invariant I5)
**Files**: `crates/node/tests/sync_sim/protocol.rs` or new test file

**Tasks**:
- [ ] 4.1: Create test where Alice and Bob have same entity with different values
- [ ] 4.2: Verify merge uses LWW (Last-Writer-Wins) based on timestamp
- [ ] 4.3: Test edge cases: same timestamp, one node has entity other doesn't
- [ ] 4.4: Verify no data loss (I5 compliance)

**Test scenarios**:
```
Scenario A: Bob has newer timestamp
  Alice: entity_1 = "old", ts=100
  Bob:   entity_1 = "new", ts=200
  After sync: Both have entity_1 = "new"

Scenario B: Alice has newer timestamp  
  Alice: entity_1 = "newer", ts=300
  Bob:   entity_1 = "old", ts=100
  After sync: Both have entity_1 = "newer"

Scenario C: Same timestamp (deterministic tiebreaker)
  Alice: entity_1 = "alice-version", ts=100
  Bob:   entity_1 = "bob-version", ts=100
  After sync: Deterministic winner (e.g., by node ID or hash)
```

**Acceptance**: All CRDT merge scenarios pass, no data loss

### Phase 5: Clean Up and Documentation
**Tasks**:
- [ ] 5.1: Remove debug prints
- [ ] 5.2: Remove unused code (old simulation protocol if replaced)
- [ ] 5.3: Add doc comments explaining the transport abstraction
- [ ] 5.4: Update any related documentation

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

## Success Criteria

1. Simulation runs **production protocol code** through `SimStream`
2. Entity counts match after sync
3. CRDT merge tests pass (I5 verified)
4. All 237+ existing tests still pass
5. No reimplemented protocol logic in simulation

---

## Notes

- Production protocol may have dependencies that are hard to mock (DeltaStore, RuntimeEnv)
- May need to extract core protocol logic into functions that are agnostic to storage backend
- Keep simulation fast - no actual network, no disk I/O
