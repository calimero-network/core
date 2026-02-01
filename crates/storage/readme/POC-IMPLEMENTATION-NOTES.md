# POC Implementation Notes: Hybrid Sync Protocol

> **Purpose**: This document captures implementation-specific details, bugs discovered, and fixes applied during the `test/tree_sync` branch development.
>
> **Status**: Branch-specific (can be archived/deleted after merge)
>
> **Branch**: `test/tree_sync`

---

## Table of Contents

1. [Implementation Phases](#implementation-phases)
2. [Bugs Discovered & Fixed](#bugs-discovered--fixed)
3. [Performance Findings](#performance-findings)
4. [Implementation Status](#implementation-status)

---

## Implementation Phases

### Phase 1: Storage Layer (COMPLETED)

Basic storage infrastructure:
- `Metadata` struct with `crdt_type` field
- `EntityIndex` for Merkle tree navigation
- Collection CRDT implementations (Counter, UnorderedMap, etc.)

### Phase 2: Hybrid Merge Architecture (COMPLETED)

Storage layer changes:
- `CrdtType` enum in metadata
- `merge_by_crdt_type_with_callback()` in Interface
- Collections auto-set their `crdt_type` on creation

### Phase 3: Network Layer Integration (COMPLETED)

Network message updates:
- `TreeLeafData` struct with metadata
- `SyncHandshake` / `SyncHandshakeResponse`
- Tree sync strategies (HashComparison, BloomFilter, etc.)

### Phase 4: Integration (COMPLETED)

Wiring it all together:
- `SyncManager` protocol negotiation
- Merge callback dispatch from tree sync
- Delta buffering during snapshot sync

### Phase 5: Optimization (COMPLETED)

Performance improvements:
- Deterministic collection IDs
- Smart concurrent branch detection
- Parallel peer dialing

### Phase 6: Delta Pruning (TODO - Separate PR)

Not in scope for this branch:
- Checkpoint creation protocol
- Delta history pruning
- Quorum-based attestation

---

## Bugs Discovered & Fixed

### Bug 1: LazyRecipient Cross-Arbiter Message Loss

**Discovery**: During three-node sync testing, Node 2 received 40 `StateDelta` messages but only processed 12.

**Root Cause**: Actix's `LazyRecipient` silently drops messages when the target arbiter is busy.

**Fix**: Replaced with explicit `tokio::sync::mpsc` channel.

**Files**: `crates/network/src/lib.rs`, `crates/node/src/network_event_processor.rs`

---

### Bug 2: Collection ID Randomization

**Discovery**: Same code on different nodes produced different collection IDs.

**Root Cause**: `Collection::new()` called `Id::random()` for unspecified IDs.

**Fix**: Introduced `new_with_field_name()` for deterministic IDs based on parent + field name.

**Files**: `crates/storage/src/collections.rs`

---

### Bug 3: Hash Mismatch Rejection

**Discovery**: Valid deltas rejected with "hash mismatch" errors.

**Root Cause**: Code expected hashes to match after applying concurrent branch deltas, but CRDT merge intentionally produces a new merged hash.

**Fix**: Trust CRDT semantics - hash divergence after merge is expected, not an error.

**Files**: `crates/node/src/delta_store.rs`

---

### Bug 4: LWW Rejecting Root Merges

**Discovery**: Root entity updates with older timestamps were rejected before CRDT merge could happen.

**Root Cause**: LWW check happened before type-aware merge dispatch.

**Fix**: Always attempt CRDT merge first for root entities.

**Files**: `crates/storage/src/interface.rs`

---

### Bug 5: Bloom Filter Hash Mismatch (P0)

**Discovery**: Bloom filter diff detection returned wrong results.

**Root Cause**: `sync_protocol.rs` used FNV-1a hash, `dag/lib.rs` used SipHash (`DefaultHasher`).

**Fix**: Both now use FNV-1a.

**Files**: `crates/dag/src/lib.rs`, `crates/node/primitives/src/sync_protocol.rs`

---

### Bug 6: Metadata Not Persisted (P0)

**Discovery**: CRDT types fell back to LWW on subsequent syncs.

**Root Cause**: Tree sync wrote entity value but not `EntityIndex` (which holds `crdt_type`).

**Fix**: Added `Index::persist_metadata_for_sync()` and call it after merge.

**Files**: `crates/storage/src/index.rs`, `crates/node/src/sync/tree_sync.rs`

---

### Bug 7: BufferedDelta Missing Fields (P0)

**Discovery**: Deltas buffered during snapshot sync couldn't be replayed.

**Root Cause**: `BufferedDelta` only stored `id`, `parents`, `hlc`, `payload` - missing `nonce` (decryption), `author_id` (sender key), `root_hash`, `events`.

**Fix**: Extended `BufferedDelta` struct with all fields.

**Files**: `crates/node/primitives/src/sync_protocol.rs`, `crates/node/src/handlers/state_delta.rs`

---

### Bug 8: Parallel Dialing Exhaustion (P1)

**Discovery**: Sync failed even when viable peers existed beyond first batch.

**Root Cause**: Parallel dialing tried first N peers, gave up if all failed.

**Fix**: Sliding window refill - keep trying until all peers exhausted.

**Files**: `crates/node/src/sync/manager.rs`

---

### Bug 9: remote_root_hash = local_root_hash (P1)

**Discovery**: Tree comparison short-circuited (thought state was identical).

**Root Cause**: Code passed `local_root_hash` instead of peer's hash from handshake.

**Fix**: Pass `peer_root_hash` from `SyncHandshakeResponse` to tree sync.

**Files**: `crates/node/src/sync/manager.rs`

---

### Bug 10: Adaptive Selection Always Returns Snapshot (P0) - Bugbot

**Discovery**: Bugbot flagged that `AdaptiveSelection` always triggered expensive Snapshot sync.

**Root Cause**: `local_entity_count` was hardcoded to `0` in `network_event.rs`. The `adaptive_select()` function returns `Snapshot` when `local_entity_count == 0` (interprets as "empty node needs bootstrap").

**Fix**: Use remote's `entity_count` as conservative estimate. If we're in the same context, counts are likely similar. True divergence (remote=1000, local=0) still triggers Snapshot correctly.

**Files**: `crates/node/src/handlers/network_event.rs`

---

### Bug 11: Dead Code - RootHashMismatch Handler (P2) - Bugbot

**Discovery**: Bugbot flagged unreachable code checking for `RootHashMismatch`.

**Root Cause**: The `apply()` function never returns `ApplyError::RootHashMismatch`. Hash mismatches are handled inside `ContextStorageApplier` using CRDT merge semantics, not error returns.

**Fix**: Removed dead code path. Hash divergence is now expected behavior (CRDT merge produces new merged state).

**Files**: `crates/node/src/handlers/state_delta.rs`

---

### Bug 12: parent_hashes HashMap Unbounded Growth (P1) - Bugbot

**Discovery**: Bugbot flagged that `parent_hashes` HashMap grows without limit.

**Root Cause**: Every applied delta adds 64 bytes to `parent_hashes`. Unlike `head_root_hashes` (which has `retain()` to prune non-heads), `parent_hashes` only grew.

**Fix**: Added `MAX_PARENT_HASH_ENTRIES` (10,000) limit. When exceeded, prunes ~10% oldest entries. 10,000 entries = ~640KB, sufficient for merge detection which mainly needs recent parent-child relationships.

**Files**: `crates/node/src/delta_store.rs`

---

## Performance Findings

### Key Finding: Peer Selection Dominates

| Phase | Time (P50) | % of Total |
|-------|-----------|------------|
| Peer Selection | 286ms | 85% |
| Key Share | 25ms | 7% |
| DAG Compare | 15ms | 4% |
| Delta Apply | 10ms | 3% |

**Insight**: Peer finding is fast (<0.2ms), but dialing is slow (150-200ms P50).

### Optimization Applied

- **Parallel dialing** with `FuturesUnordered`
- **Connection state tracking** for RTT-based peer selection
- **Recent peer cache** to prefer known-good peers

### Remaining Bottleneck

Dialing latency is fundamentally limited by:
- TCP 3-way handshake (~50ms on LAN)
- TLS negotiation (~30ms)
- libp2p protocol negotiation (~20ms)

Future: Connection pooling, keep-alive tuning.

---

## Implementation Status

| Feature | Status |
|---------|--------|
| Protocol Negotiation | ✅ Done |
| TreeLeafData with Metadata | ✅ Done |
| Built-in CRDT Merge | ✅ Done |
| WASM Custom Type Merge | ⚠️ Deferred (LWW fallback) |
| Parallel Dialing | ✅ Done |
| Checkpoint Deltas | ✅ Done |
| Bloom Filter Metadata | ✅ Done |
| Metadata Persistence | ✅ Done |
| HybridSync v2 | ✅ Done |
| Payload Compression | 🔲 Future |

---

## Test Evidence

### E2E Workflows Run

| Workflow | Nodes | Result |
|----------|-------|--------|
| `three-node-sync.yml` | 3 | ✅ Pass |
| `lww-conflict-resolution.yml` | 3 | ✅ Pass |
| `restart-sync.yml` | 2 | ✅ Pass |
| `fresh-node-catchup.yml` | 3 | ✅ Pass |

### Unit Test Coverage

- 35 tests in `sync_protocol_negotiation.rs`
- 14 tests in `sync_integration.rs`
- 17 tests in `concurrent_merge.rs`
- 21 tests in `merge_integration.rs`

---

*Created: February 1, 2026*  
*Branch: test/tree_sync*
