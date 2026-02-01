# Issue 006: Delta Buffering During State Sync

**Priority**: P0 (Safety Critical)  
**CIP Section**: §5 - Delta Handling During Sync  
**Invariant**: I6 (Liveness Guarantee)

## Summary

During state-based synchronization, incoming deltas MUST be buffered and replayed after sync completes. Dropping deltas violates liveness guarantees.

## Problem

While a node is receiving state (HashComparison, BloomFilter, etc.), other nodes continue producing deltas. If these are dropped:
- Data loss occurs
- Convergence fails
- Node falls behind again immediately

## Solution

### SyncContext with Buffer

```rust
pub struct SyncContext {
    pub state: SyncState,
    pub buffered_deltas: VecDeque<BufferedDelta>,
    pub buffer_capacity: usize,
    pub sync_start_time: Instant,
}

pub struct BufferedDelta {
    pub id: [u8; 32],
    pub parents: Vec<[u8; 32]>,
    pub hlc: HybridTimestamp,
    pub nonce: [u8; 24],      // For decryption
    pub author_id: PublicKey, // Sender key
    pub root_hash: [u8; 32],  // Expected root after apply
    pub payload: Vec<u8>,
    pub events: Vec<Event>,
}
```

### Buffer Lifecycle

```
┌───────────────────────────────────────────────────────────┐
│ SYNC IN PROGRESS                                          │
│                                                           │
│   [State transfer]  ◄──── Incoming deltas                │
│         │                      │                         │
│         │                      ▼                         │
│         │             [BufferedDelta queue]              │
│         │                      │                         │
│         ▼                      │                         │
│   [State applied]              │                         │
│         │                      │                         │
│         └──────────► [Replay buffered deltas via DAG] ◄──┘
│                                                           │
└───────────────────────────────────────────────────────────┘
```

## Implementation Tasks

- [ ] Define `SyncContext` struct
- [ ] Define `BufferedDelta` with ALL required fields
- [ ] Implement `buffer_delta()` method
- [ ] Implement `replay_buffered_deltas()` via DAG insertion
- [ ] Handle buffer overflow (should not drop - log warning)
- [ ] Add metrics for buffer size and replay count

## Critical: Replay via DAG

Buffered deltas MUST be replayed via DAG insertion (causal order), NOT by HLC timestamp sorting:

```rust
// CORRECT: Insert into DAG, apply in causal order
for delta in buffered_deltas {
    dag_store.insert(delta)?;
}
dag_store.apply_pending()?;

// WRONG: Sort by HLC and apply
// buffered_deltas.sort_by_key(|d| d.hlc);  // NO!
```

## Acceptance Criteria

- [ ] Deltas arriving during sync are buffered
- [ ] All fields required for replay are captured
- [ ] Buffer survives sync completion
- [ ] Replay uses DAG insertion (causal order)
- [ ] No deltas are dropped (log if buffer approaches limit)
- [ ] Metrics track buffer usage

## Files to Modify

- `crates/node/src/sync/context.rs` (new)
- `crates/node/src/handlers/state_delta.rs`
- `crates/node/primitives/src/sync_protocol.rs`

## POC Reference

See Bug 7 (BufferedDelta missing fields) in [POC-IMPLEMENTATION-NOTES.md](../POC-IMPLEMENTATION-NOTES.md)
