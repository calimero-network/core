# Architecture Decisions: Hybrid Sync Protocol

> **Purpose**: This document captures the key implementation decisions made while building the hybrid sync protocol. Each decision includes context, options considered, the choice made, and consequences.
>
> **Audience**: Engineers implementing, reviewing, or maintaining sync code.

---

## Table of Contents

1. [Network Event Delivery](#1-network-event-delivery)
2. [Bloom Filter Hash Function](#2-bloom-filter-hash-function)
3. [Snapshot Boundary Representation](#3-snapshot-boundary-representation)
4. [Wire Protocol Versioning](#4-wire-protocol-versioning)
5. [Parallel Peer Dialing](#5-parallel-peer-dialing)
6. [CRDT Merge Dispatch](#6-crdt-merge-dispatch)
7. [Metadata Persistence in Tree Sync](#7-metadata-persistence-in-tree-sync)
8. [Delta Buffering During Snapshot](#8-delta-buffering-during-snapshot)

---

## 1. Network Event Delivery

### Context

The `NetworkManager` (libp2p) runs on a separate Actix arbiter from the `NodeManager`. Network events (gossip messages, stream data) need to cross this boundary reliably.

### Options Considered

| Option | Pros | Cons |
|--------|------|------|
| **A: LazyRecipient** | Built into Actix, simple API | Silently drops messages when arbiter is busy; no backpressure |
| **B: tokio::mpsc channel** | Explicit backpressure, reliable delivery, async-native | Manual wiring, need to spawn receiver task |
| **C: Actix Broker** | Built-in pub/sub | Still Actix-bound, same arbiter issues |

### Decision

**Option B: Explicit `tokio::sync::mpsc` channel**

### Rationale

- `LazyRecipient` was observed silently dropping messages under load (no errors, just lost events)
- Channel provides explicit backpressure (bounded channel blocks sender)
- Decouples from Actix lifecycle - works even if arbiter is restarting
- Easy to add metrics (channel depth, send latency)

### Consequences

- Added `NetworkEventChannel` type alias
- Created `NetworkEventProcessor` to bridge channel → NodeManager
- **Future**: Consider migrating away from Actix entirely (see RFC-ACTIX-NETWORK-ARCHITECTURE.md)

### Files Changed

- `crates/network/src/lib.rs` - Channel creation
- `crates/node/src/network_event_processor.rs` - New bridge component
- `crates/node/src/run.rs` - Wiring

---

## 2. Bloom Filter Hash Function

### Context

Bloom filters are used to quickly detect which delta IDs the remote peer is missing. The filter is created in `sync_protocol.rs` and queried in `dag/lib.rs`.

### Options Considered

| Option | Pros | Cons |
|--------|------|------|
| **A: std::hash::DefaultHasher (SipHash)** | Standard library, cryptographically stronger | Different implementations may vary; overkill for bloom filter |
| **B: FNV-1a** | Fast, deterministic, widely used for bloom filters | Not cryptographic (doesn't matter here) |
| **C: xxHash** | Very fast | External dependency |

### Decision

**Option B: FNV-1a in both locations**

### Rationale

- Bloom filters don't need cryptographic hashing
- FNV-1a is simple to implement identically in multiple places
- **Critical**: Both sides MUST use the same hash function or bit positions won't match
- We discovered a bug where `sync_protocol.rs` used FNV-1a but `dag/lib.rs` used SipHash

### Consequences

- Added `bloom_hash()` function to `dag/lib.rs` using FNV-1a
- Matches `DeltaIdBloomFilter::hash_fnv1a()` in `sync_protocol.rs`
- Must keep these in sync (consider extracting to shared crate)

### Files Changed

- `crates/dag/src/lib.rs` - Added `bloom_hash()` function
- `crates/node/primitives/src/sync_protocol.rs` - Reference implementation

---

## 3. Snapshot Boundary Representation

### Context

After snapshot sync, the DAG doesn't have the delta history. When new deltas arrive referencing pre-snapshot parents, the DAG would reject them as "missing parents".

### Options Considered

| Option | Pros | Cons |
|--------|------|------|
| **A: Fake delta stubs** | Quick hack, works | Pollutes DAG with fake data; confusing semantics |
| **B: Special "checkpoint" flag in delta** | Clean protocol concept; self-documenting | Requires wire format change |
| **C: Separate checkpoint table** | Clean separation | More complex; need to check two places |

### Decision

**Option B: `DeltaKind::Checkpoint` enum variant**

### Rationale

- Checkpoints are a first-class protocol concept, not a hack
- `kind: Checkpoint` is self-documenting in logs and debugging
- Backward compatible via `#[serde(default)]` (old deltas default to `Regular`)
- Clean API: `CausalDelta::checkpoint()` constructor

### Consequences

- Added `DeltaKind` enum to `calimero_dag::CausalDelta`
- Replaced `add_snapshot_boundary_stubs()` with `add_snapshot_checkpoints()`
- Checkpoints have empty payload and cannot be replayed

### Files Changed

- `crates/dag/src/lib.rs` - `DeltaKind` enum, `checkpoint()` constructor
- `crates/node/src/delta_store.rs` - `add_snapshot_checkpoints()`

---

## 4. Wire Protocol Versioning

### Context

The sync wire protocol evolved during development. `TreeLeafData` now includes `Metadata`, `BufferedDelta` has more fields, etc. Mixed-version clusters would crash on deserialization.

### Options Considered

| Option | Pros | Cons |
|--------|------|------|
| **A: No versioning** | Simple | Crashes on mixed clusters |
| **B: Version in handshake** | Clean negotiation; reject incompatible peers | Requires version bump discipline |
| **C: Self-describing format (e.g., protobuf)** | Maximum flexibility | Heavy dependency; overkill |

### Decision

**Option B: Explicit version in `SyncProtocolVersion::HybridSync { version: u8 }`**

### Rationale

- Handshake already exists - just add version field
- Protocol negotiation rejects incompatible versions early (clear error)
- Lightweight - just a u8

### Consequences

- Bumped `HybridSync` from v1 to **v2**
- `SyncCapabilities::protocols_compatible()` checks version match
- **Breaking**: v1 and v2 nodes cannot sync (by design)

### Files Changed

- `crates/node/primitives/src/sync_protocol.rs` - Version bump

---

## 5. Parallel Peer Dialing

### Context

Finding a viable sync peer can be slow. If we try peers sequentially and the first few fail, P99 latency spikes.

### Options Considered

| Option | Pros | Cons |
|--------|------|------|
| **A: Sequential dialing** | Simple, predictable resource usage | Slow when first peers fail |
| **B: Parallel all peers** | Fastest possible | Wastes resources; many cancelled dials |
| **C: Parallel with limit + sliding window** | Fast; bounded resource usage | More complex |

### Decision

**Option C: `FuturesUnordered` with sliding window refill**

### Rationale

- Start 3 dials concurrently (configurable)
- First success wins, others are cancelled
- If all 3 fail, refill window with next batch of peers
- Continues until success or all peers exhausted

### Consequences

- Uses `tokio::stream::FuturesUnordered` for true concurrency
- `ParallelDialConfig` controls `max_concurrent`, `dial_timeout_ms`
- `ParallelDialTracker` collects metrics on dial attempts
- Sliding window ensures we don't give up after just N failures

### Files Changed

- `crates/node/src/sync/dial_tracker.rs` - Tracker implementation
- `crates/node/src/sync/manager.rs` - Integration in `perform_interval_sync()`

---

## 6. CRDT Merge Dispatch

### Context

When tree sync receives an entity, it needs to merge it with local state using the correct CRDT semantics (Counter should sum, not LWW).

### Options Considered

| Option | Pros | Cons |
|--------|------|------|
| **A: Always LWW** | Simple | Data loss for Counters, Maps, etc. |
| **B: Dispatch based on `crdt_type` in metadata** | Correct merge semantics | Need to propagate metadata over wire |
| **C: Infer type from value bytes** | No wire changes | Fragile; can't distinguish types reliably |

### Decision

**Option B: Include `Metadata` (with `crdt_type`) in `TreeLeafData` wire format**

### Rationale

- `crdt_type` is already stored in `EntityIndex.metadata`
- Wire format just needs to carry it: `TreeLeafData { key, value, metadata }`
- `Interface::merge_by_crdt_type_with_callback()` handles dispatch

### Consequences

- `TreeLeafData` struct added to wire protocol
- `handle_tree_node_request` reads `EntityIndex` and includes metadata
- All tree sync strategies use `apply_entity_with_merge()` for correct dispatch

### Files Changed

- `crates/node/primitives/src/sync.rs` - `TreeLeafData` struct
- `crates/node/src/sync/manager.rs` - Metadata population
- `crates/node/src/sync/tree_sync.rs` - `apply_entity_with_merge()`
- `crates/storage/src/interface.rs` - Made `merge_by_crdt_type_with_callback` public

---

## 7. Metadata Persistence in Tree Sync

### Context

Tree sync writes entity values to storage, but the `crdt_type` in `EntityIndex.metadata` also needs to be persisted. Otherwise, subsequent merges fall back to LWW.

### Options Considered

| Option | Pros | Cons |
|--------|------|------|
| **A: Rely on storage layer auto-persist** | Less code | Storage layer doesn't auto-persist on external writes |
| **B: Explicit `Index::persist_metadata_for_sync()` call** | Clear, explicit | Extra API surface |

### Decision

**Option B: Explicit API for sync to persist metadata**

### Rationale

- Tree sync bypasses normal entity write path (uses `store_handle.put()` directly)
- Normal writes go through `Collection::insert()` which handles metadata
- Sync needs explicit call: `Index::persist_metadata_for_sync(context_id, entity_id, metadata)`

### Consequences

- Added `Index::persist_metadata_for_sync()` public API
- `apply_entity_with_merge()` calls this after writing value
- Ensures `crdt_type` survives for future merges

### Files Changed

- `crates/storage/src/index.rs` - New public API
- `crates/node/src/sync/tree_sync.rs` - Call after merge

---

## 8. Delta Buffering During Snapshot

### Context

During snapshot sync, new deltas may arrive via gossip. These need to be buffered and replayed after snapshot completes.

### Options Considered

| Option | Pros | Cons |
|--------|------|------|
| **A: Drop incoming deltas** | Simple | Data loss if snapshot is slow |
| **B: Buffer minimal info (id, parents, hlc, payload)** | Low memory | Can't decrypt/verify without nonce, author |
| **C: Buffer all fields needed for replay** | Correct replay | Higher memory |

### Decision

**Option C: `BufferedDelta` includes all fields for complete replay**

### Rationale

- Delta replay needs: `nonce` (for decryption), `author_id` (for sender key), `root_hash` (for verification), `events` (optional)
- Without these, buffered deltas can't be processed after snapshot
- Memory overhead is acceptable (bounded buffer size, short duration)

### Consequences

- `BufferedDelta` struct extended with: `nonce`, `author_id`, `root_hash`, `events`
- `state_delta.rs` populates all fields when buffering
- Buffer has max capacity (`DeltaBuffer::new(capacity, sync_start_hlc)`)

### Files Changed

- `crates/node/primitives/src/sync_protocol.rs` - Extended struct
- `crates/node/src/handlers/state_delta.rs` - Populate all fields

---

## Summary: Key Principles

1. **Explicit over implicit** - Channels over LazyRecipient, explicit metadata persist over auto-magic
2. **Protocol-level concepts** - Checkpoints as first-class deltas, not hacks
3. **Correctness over simplicity** - Buffer all fields, dispatch by CRDT type
4. **Bounded resources** - Parallel dialing with limits, bounded delta buffer
5. **Version early** - Wire protocol versioning prevents silent corruption

---

*Created: February 1, 2026*  
*Branch: test/tree_sync*
