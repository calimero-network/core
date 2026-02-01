# Issue 009: SubtreePrefetch Sync Strategy

**Priority**: P2  
**CIP Section**: Appendix B - Protocol Selection Matrix  
**Depends On**: 007-hash-comparison-sync

## Summary

Implement subtree prefetch for deep trees with localized changes. Fetches entire subtrees when divergence is detected, reducing round trips.

## When to Use

- `max_depth > 3`
- `divergence < 20%`
- Changes are clustered in subtrees

## Protocol Flow

```
Initiator                          Responder
    │                                   │
    │ ──── SubtreePrefetchRequest ────► │
    │      { subtree_roots: [...] }     │
    │                                   │
    │ ◄──── SubtreePrefetchResponse ─── │
    │      { subtrees: [...] }          │
    │                                   │
    │ (CRDT merge all entities)         │
    │                                   │
```

## Messages

```rust
pub struct SubtreePrefetchRequest {
    pub subtree_roots: Vec<[u8; 32]>,
    pub max_depth: Option<usize>,
}

pub struct SubtreePrefetchResponse {
    pub subtrees: Vec<SubtreeData>,
}

pub struct SubtreeData {
    pub root_id: [u8; 32],
    pub entities: Vec<TreeLeafData>,
}
```

## Algorithm

1. Compare root hashes
2. Identify differing top-level subtrees
3. Request entire subtrees (not just nodes)
4. Receive all entities in subtree
5. CRDT merge each entity

## Implementation Tasks

- [ ] Define SubtreePrefetch messages
- [ ] Implement subtree serialization
- [ ] Detect clustered changes (heuristic)
- [ ] Fetch complete subtrees in single request
- [ ] Apply via CRDT merge
- [ ] Limit prefetch depth to avoid over-fetching

## Trade-offs

| Aspect | HashComparison | SubtreePrefetch |
|--------|----------------|-----------------|
| Round trips | O(depth) | O(1) per subtree |
| Data transfer | Minimal | May over-fetch |
| Best for | Scattered changes | Clustered changes |

## Acceptance Criteria

- [ ] Subtrees are fetched completely
- [ ] Metadata included for all entities
- [ ] CRDT merge used
- [ ] Depth limit prevents excessive transfer
- [ ] Fewer round trips than HashComparison for deep trees

## Files to Modify

- `crates/node/src/sync/subtree_sync.rs` (new)
- `crates/node/primitives/src/sync.rs`

## POC Reference

See tree_sync.rs subtree handling in POC branch.
