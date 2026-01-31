# Branch Checkpoint: test/tree_sync

**Date**: January 31, 2026  
**Commits**: ~50 commits, +26,700 lines, -423 lines  
**Status**: ✅ Phase 4 Complete - Ready for Review

---

## Executive Summary

This branch implements a **hybrid state synchronization protocol** for Calimero nodes. Key achievements:

| Metric | Before | After |
|--------|--------|-------|
| Peer finding latency | 286-422ms P50 | **0.04-0.12ms** P50 |
| Connection reuse | 0% | **100%** (warm state) |
| Protocol negotiation | None | **HybridSync v1** negotiated |
| Merge callback | Not wired | **WASM-dispatchable** |
| Instrumentation | Basic | **Comprehensive** (phases, dial, metrics) |

---

## Document Organization

### What Goes in the CIP

The **CIP-sync-protocol.md** should contain:

1. **Protocol Specification** (already present)
   - Sync protocol types (DeltaSync, HashComparison, Snapshot, etc.)
   - Message formats (SyncHandshake, TreeNodeRequest, etc.)
   - Protocol negotiation flow
   - Merge callback interface

2. **Motivation & Use Cases** (already present)
   - Why hybrid sync is needed
   - Fresh node bootstrap problem
   - Concurrent conflict resolution

3. **Implementation Phases** (Appendices A-N)
   - These are COMPLETE and should remain
   - They document the canonical implementation

**What to REMOVE from CIP**:
- Excessive benchmark data (move to BENCHMARK-RESULTS-2026-01.md)
- Investigation logs (keep in DEEP-SYNC-ANALYSIS.md)
- Debug/troubleshooting notes

### Supporting Documentation (Keep Separate)

| Document | Purpose | Audience |
|----------|---------|----------|
| `network-sync.md` | High-level protocol overview | Developers |
| `PEER-FINDING-ANALYSIS.md` | Phase 1 investigation | Internal/Research |
| `DIAL-OPTIMIZATION-ANALYSIS.md` | Phase 2 investigation | Internal/Research |
| `BENCHMARK-RESULTS-2026-01.md` | Current benchmark data | QA/Performance |
| `DECISION-LOG.md` | Architectural decisions | Architects |
| `PRODUCTION-MONITORING.md` | PromQL/Grafana | Operators |
| `DEEP-SYNC-ANALYSIS.md` | Comprehensive research | Internal |
| `EDGE-CASE-BENCHMARK-RESULTS.md` | Edge case data | QA/Performance |

### Documentation Index Update Needed

The `DOCUMENTATION_INDEX.md` is good but should clarify:
- CIP = Standards track (for protocol approval)
- Other docs = Supporting evidence

---

## What We Got Right ✅

### 1. Protocol Negotiation (Phase 4)
- `SyncHandshake` → `SyncHandshakeResponse` with negotiated protocol
- Clean separation between handshake and key share
- Extensible for future protocol versions

### 2. Connection Reuse
- 100% reuse rate in steady state
- `was_connected_initially` tracking
- RTT-based peer sorting

### 3. Instrumentation Design
- Clean separation: `PEER_FIND_PHASES` vs `PEER_DIAL_BREAKDOWN`
- Per-phase timing (`SYNC_PHASE_BREAKDOWN`)
- Prometheus metrics + structured logs

### 4. Benchmark Workflows
- Repeatable merobox YAML tests
- Edge case coverage (churn, partition, storm)
- Strategy comparison framework

### 5. Gossipsub Tuning
- Reduced backoff from 60s to 5s
- Faster mesh reformation
- Fixes restart recovery

---

## What We Should Review / Potential Issues ⚠️

### 1. LazyRecipient/Actix Architecture

**Problem Encountered**: Cross-arbiter message loss with `LazyRecipient<NetworkEvent>`.

**Current Fix**: Dedicated `mpsc` channel + bridge actor.

**Technical Debt**:
- We have TWO message paths now (Actix actors + channel)
- `NetworkEventBridge` is a workaround, not a clean solution
- The bridge runs as a separate tokio task, adding complexity

**Recommendation for Next Quarter**:
- Consider full migration away from Actix to pure tokio
- Or: Deep dive into Actix to understand LazyRecipient failures
- Document the actual failure mode (silent drops? backpressure?)

### 2. Merge Callback Not Fully Exercised

**Current State**:
- `get_merge_callback()` is implemented
- `handle_tree_sync_with_callback()` passes it to strategies
- **But**: Entity type metadata is not stored, so callback can't determine which merge function to call

**Technical Debt**:
- The callback defaults to LWW merge
- Full CRDT merge requires storage changes (entity type tracking)

**Recommendation**:
- Add entity type to storage schema
- Or: Accept LWW for state sync (delta sync still uses proper CRDT merge)

### 3. Dead Code in tree_sync.rs

**Potential Issues**:
- `bloom_filter_sync`, `subtree_prefetch_sync`, `level_wise_sync` are implemented
- They're wired into `handle_tree_sync_with_callback()`
- But: They're only used when `--force-state-sync` is passed
- In production, `Adaptive` strategy defaults to `HashComparison`

**Recommendation**:
- Either remove unused strategies
- Or: Add proper strategy selection heuristics in `Adaptive`
- Or: Mark as `#[allow(dead_code)]` with comment

### 4. Snapshot Boundary Stubs

**Current Behavior**: After snapshot sync, we create "boundary stubs" in the DAG.

**Concern**: These are fake deltas that allow DAG traversal to work, but:
- They have no actual content
- They're a workaround for DAG design assumptions

**Recommendation**:
- Document this clearly in CIP
- Consider: Should snapshot sync bypass DAG entirely?

### 5. Test Coverage Gaps

**What's Tested**:
- E2E workflows (merobox)
- Unit tests for dial_tracker
- Integration tests for sync flow

**What's NOT Tested**:
- Parallel dialing (infrastructure only, not integrated)
- Catch-up mode under real churn
- Multi-context sync behavior

---

## Code That May Be Unnecessary

### 1. `ParallelDialTracker` and `ParallelDialConfig`
- **File**: `crates/node/src/sync/dial_tracker.rs`
- **Status**: Implemented with tests, NOT INTEGRATED
- **Issue**: Infrastructure only - never called from main sync path
- **Decision**: Keep for now (documented as "infrastructure only" in DECISION-LOG.md)

### 2. Bloom Filter Strategy (for state sync)
- **File**: `crates/node/src/sync/tree_sync.rs`
- **Status**: Implemented, only used with `--force-state-sync bloom`
- **Issue**: Adaptive strategy never selects it
- **Decision**: Keep - useful for benchmarking, may be used later

### 3. `DeltaBuffer` (Delta Buffering)
- **File**: `crates/node/src/sync/manager.rs` (if present)
- **Status**: Partially implemented, TODO was cancelled
- **Issue**: Snapshot sync doesn't buffer incoming deltas during transfer
- **Decision**: Acceptable - short sync windows unlikely to lose deltas

### 4. Multiple Peer Find Strategies (A0-A5)
- **File**: `crates/node/src/sync/peer_finder.rs`
- **Status**: All implemented, `baseline` is default
- **Issue**: Other strategies rarely needed (finding is <1ms)
- **Decision**: Keep - useful for benchmarking, minimal overhead

---

## Architecture Suggestions for Next Quarter

### 1. Actix → Pure Tokio Migration

**Current Pain Points**:
- `LazyRecipient` cross-arbiter issues
- Mixed async runtimes (Actix + tokio)
- Complex message bridging

**Proposal**:
- Migrate `NodeManager` from Actix actor to tokio task
- Use channels instead of Actix messages
- Keep Actix only where essential (server handlers?)

**Effort**: Large (2-4 weeks)  
**Risk**: High (core refactor)  
**Benefit**: Simpler mental model, fewer runtime surprises

### 2. Network Layer Abstraction

**Current Pain Points**:
- Direct libp2p usage scattered
- Gossipsub configuration hardcoded
- Difficult to mock for testing

**Proposal**:
- Create `NetworkService` trait
- Hide libp2p behind interface
- Allow mock implementations for testing

**Effort**: Medium (1-2 weeks)  
**Risk**: Medium  
**Benefit**: Better testability, easier libp2p upgrades

### 3. Structured Logging / OpenTelemetry

**Current State**:
- Custom log markers (`PEER_FIND_PHASES`, etc.)
- Manual parsing via shell scripts
- Prometheus metrics separate from logs

**Proposal**:
- Adopt structured JSON logging
- Add OpenTelemetry tracing spans
- Correlate logs/metrics/traces

**Effort**: Medium (1-2 weeks)  
**Risk**: Low  
**Benefit**: Better observability, standard tooling

### 4. Entity Type Metadata in Storage

**Current Gap**:
- Storage doesn't track entity types (Counter, Map, Register, etc.)
- Merge callback can't dispatch to correct merge function
- Falls back to LWW

**Proposal**:
- Add `entity_type: EntityTypeId` to storage schema
- Map types in `MergeRegistry`
- Enable proper CRDT merge during state sync

**Effort**: Medium (1-2 weeks)  
**Risk**: Medium (schema change)  
**Benefit**: Full CRDT semantics in all sync paths

---

## Files Changed Summary

### Core Sync Implementation
- `crates/node/src/sync/manager.rs` - Main sync orchestration
- `crates/node/src/sync/config.rs` - Configuration
- `crates/node/src/sync/peer_finder.rs` - Peer discovery
- `crates/node/src/sync/dial_tracker.rs` - NEW: Dial instrumentation
- `crates/node/src/sync/tree_sync.rs` - Tree sync strategies
- `crates/node/src/sync/metrics.rs` - Prometheus metrics

### Network Layer
- `crates/network/src/behaviour.rs` - Gossipsub config
- `crates/network/src/lib.rs` - LazyRecipient wrapper
- `crates/node/src/network_event_processor.rs` - Event bridge

### Protocol Messages
- `crates/node/primitives/src/sync.rs` - Sync messages
- `crates/node/primitives/src/sync_protocol.rs` - Protocol types

### E2E Tests (19 workflow files)
- `workflows/sync/*.yml`

### Documentation (15 markdown files)
- `crates/storage/readme/*.md`

---

## Recommended Actions Before Merge

### Must Do
1. [ ] Squash fix commits (multiple "fix" commits for same issue)
2. [ ] Review `#[allow(dead_code)]` annotations
3. [ ] Run full E2E test suite
4. [ ] Update CHANGELOG.md

### Should Do
1. [ ] Trim CIP appendices (move verbose data to separate docs)
2. [ ] Add deprecation comment to LazyRecipient usage
3. [ ] Document entity type limitation for merge callback

### Nice to Have
1. [ ] Add architecture diagram to CIP
2. [ ] Create "sync troubleshooting" guide
3. [ ] Add more edge case E2E tests

---

## Conclusion

This branch represents a **significant improvement** in sync reliability and performance. The core protocol negotiation and instrumentation are solid. 

**Technical debt** exists around:
1. Actix/channel duality (workaround, not fix)
2. Merge callback entity types (incomplete)
3. Unused parallel dialing infrastructure

**Recommendations**:
1. Merge as-is for the sync improvements
2. Track Actix migration as separate initiative
3. Entity type metadata as follow-up work

---

*This checkpoint created: January 31, 2026*  
*Branch: test/tree_sync*  
*Reviewed by: Automated Analysis*
