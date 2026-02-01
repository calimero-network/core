# Issue 005: Delta Sync Implementation

**Priority**: P1  
**CIP Section**: §4 - State Machine (DELTA SYNC branch)  
**Depends On**: 003, 004

## Summary

Implement delta-based synchronization for scenarios where few deltas are missing and DAG heads are known.

## When to Use

- Missing < threshold deltas (configurable, default ~50)
- Parent delta IDs are known
- Real-time updates with small gaps

## Protocol Flow

```
Initiator                          Responder
    │                                   │
    │ ──── DeltaSyncRequest ──────────► │
    │      { missing_ids: [...] }       │
    │                                   │
    │ ◄──── DeltaSyncResponse ───────── │
    │      { deltas: [...] }            │
    │                                   │
    │ (Apply deltas in causal order)    │
    │                                   │
```

## Messages

```rust
pub struct DeltaSyncRequest {
    pub missing_delta_ids: Vec<[u8; 32]>,
}

pub struct DeltaSyncResponse {
    pub deltas: Vec<CausalDelta>,
}
```

## Implementation Tasks

- [ ] Define DeltaSyncRequest/Response messages
- [ ] Implement delta lookup in DAG store
- [ ] Verify causal order before sending (parents first)
- [ ] Apply received deltas via WASM runtime
- [ ] Handle missing parent errors (trigger state-based sync)
- [ ] Add configurable `DELTA_SYNC_THRESHOLD`

## Delta Application

Deltas MUST be applied:
1. In causal order (parents before children)
2. Via WASM runtime (operations replayed)
3. With root hash verification

## Acceptance Criteria

- [ ] Can request specific deltas by ID
- [ ] Deltas arrive in causal order
- [ ] Missing parent triggers escalation to state-based sync
- [ ] Applied deltas update local root hash
- [ ] Performance: O(missing) network round trips

## Files to Modify

- `crates/node/src/sync/delta_sync.rs` (new)
- `crates/node/src/sync/manager.rs`
- `crates/dag/src/lib.rs`

## POC Reference

See existing delta sync logic in `crates/node/src/handlers/state_delta.rs`
