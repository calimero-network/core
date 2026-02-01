# Issue 010: LevelWise Sync Strategy

**Priority**: P2  
**CIP Section**: Appendix B - Protocol Selection Matrix  
**Depends On**: 007-hash-comparison-sync

## Summary

Implement level-by-level breadth-first synchronization for wide, shallow trees.

## When to Use

- `max_depth <= 2`
- Wide trees with many children at each level
- Changes scattered across siblings

## Protocol Flow

```
Initiator                          Responder
    │                                   │
    │ ──── LevelWiseRequest ──────────► │
    │      { level: 0 }                 │
    │                                   │
    │ ◄──── LevelWiseResponse ───────── │
    │      { nodes at level 0 }         │
    │                                   │
    │ (Compare hashes, identify diff)   │
    │                                   │
    │ ──── LevelWiseRequest ──────────► │
    │      { level: 1, parent_ids }     │
    │                                   │
    │ ◄──── LevelWiseResponse ───────── │
    │      { nodes at level 1 }         │
    │                                   │
    │ (Continue until leaves)           │
    │                                   │
```

## Messages

```rust
pub struct LevelWiseRequest {
    pub level: usize,
    pub parent_ids: Option<Vec<[u8; 32]>>,
}

pub struct LevelWiseResponse {
    pub level: usize,
    pub nodes: Vec<LevelNode>,
}

pub struct LevelNode {
    pub id: [u8; 32],
    pub hash: [u8; 32],
    pub parent_id: Option<[u8; 32]>,
    pub leaf_data: Option<TreeLeafData>,
}
```

## Algorithm

1. Request all nodes at level 0 (root children)
2. Compare hashes with local
3. For differing nodes:
   - If leaf: receive entity
   - If internal: request next level
4. Process level-by-level until complete

## Implementation Tasks

- [ ] Define LevelWise messages
- [ ] Implement breadth-first traversal
- [ ] Track which parents have differing children
- [ ] Batch requests by level
- [ ] Apply entities via CRDT merge

## Trade-offs

| Aspect | HashComparison | LevelWise |
|--------|----------------|-----------|
| Round trips | O(depth) | O(depth) |
| Messages per round | 1 | Many (batched) |
| Best for | Deep trees | Wide shallow trees |

## Acceptance Criteria

- [ ] Processes all levels correctly
- [ ] Only fetches differing subtrees
- [ ] Batches requests efficiently
- [ ] CRDT merge for all entities
- [ ] Handles very wide levels (100+ children)

## Files to Modify

- `crates/node/src/sync/level_sync.rs` (new)
- `crates/node/primitives/src/sync.rs`

## POC Reference

See tree_sync.rs level-wise handling in POC branch.
