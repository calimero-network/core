# Week 1 Status: calimero-protocols Implementation

## ✅ Completed (40% of Week 1)

### Task 1: SecureStream (DONE - 3 hours)
- ✅ Ported secure_stream.rs (856 lines)
- ✅ Ported stream.rs as private helpers (85 lines)
- ✅ Ported tracking.rs (Sequencer, SyncState) (143 lines)
- ✅ helpers::send/recv are PRIVATE (can't bypass auth!)
- ✅ Compiles cleanly
- ✅ 4/4 tests passing

**Total**: 1,084 lines of working, secure stream infrastructure

---

## ⏳ In Progress (60% remaining)

### Task 2: Gossipsub State Delta (TODO - 4 hours)
**Status**: Stub only (needs full port + refactoring)

**Challenge**: 765 lines with heavy coupling to:
- NodeClients (context + node clients)
- NodeState (delta_stores, blob_cache)
- DeltaStore (per-context DAG)
- Actor infrastructure

**Approach**: Will need to extract business logic and make stateless

---

### Task 3-5: P2P Protocols (WIP - needs refactoring)
**Status**: Copied but doesn't compile

**Files**:
- delta_request.rs (419 lines) - `impl SyncManager` methods
- blob_request.rs (262 lines) - `impl SyncManager` methods
- key_exchange.rs (112 lines) - `impl SyncManager` methods

**Problem**: All implemented as `SyncManager` methods (old architecture)

**Solution**: Extract to free functions:
```rust
// Before (coupled):
impl SyncManager {
    pub async fn handle_delta_request(&self, stream, delta_id) { ... }
}

// After (stateless):
pub async fn handle_delta_request(
    stream: &mut SecureStream,
    delta_id: [u8; 32],
    delta_store: &DeltaStore,  // Injected!
) -> Result<()> { ... }
```

---

## Timeline Update

**Original Estimate**: Week 1 = 12 hours (5 days @ 2-3 hrs/day)

**Actual Progress**:
- ✅ Day 1-2: SecureStream (3 hours) - DONE
- ⏳ Day 3-5: Protocols (9 hours) - IN PROGRESS

**Revised Estimate**:
- Need 2-3 more days of focused work to complete Week 1
- Main blocker: Making protocols stateless (requires careful refactoring)

---

## Blockers & Decisions Needed

### Blocker 1: State Delta Complexity

**Issue**: state_delta.rs is 765 lines with complex flow:
1. Validate context + identity
2. Decrypt artifact
3. Get/create DeltaStore
4. Add to DAG
5. Request missing parents
6. Execute event handlers
7. Emit WebSocket events

**Options**:
A. Port as-is, refactor later (faster, messy)
B. Refactor to stateless now (slower, cleaner)
C. Skip for now, focus on simpler protocols first

**Recommendation**: Option C - Get the simple protocols working first, prove the architecture works, then tackle state_delta.

### Blocker 2: SyncManager Coupling

**Issue**: P2P protocols are `impl SyncManager` methods, deeply coupled.

**Solution**: Extract to free functions with injected dependencies.

**Estimate**: 2-3 hours per protocol (6-9 hours total)

---

## What Works Right Now

✅ **SecureStream**: Fully functional, tested, ready to use
✅ **Crate Structure**: Clean separation (gossipsub/, p2p/, stream/)
✅ **Dependencies**: No actors, minimal coupling
✅ **Foundation**: Solid base for building protocols

---

## What's Next

**Immediate** (choose one):
1. Continue porting P2P protocols (refactor to stateless)
2. Port state_delta (biggest, most complex)
3. Pause and create design doc for stateless APIs

**My Recommendation**: 
- Finish P2P protocols first (smaller, simpler)
- Prove the stateless architecture works
- Then tackle state_delta with confidence

**Your Call**: Keep going or pause for design review?

