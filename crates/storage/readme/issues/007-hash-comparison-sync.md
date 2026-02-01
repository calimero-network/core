# Issue 007: HashComparison Sync Strategy

**Priority**: P0 (Primary Strategy)  
**CIP Section**: §4 - State Machine (STATE-BASED branch)  
**Depends On**: 001, 003, 004

## Summary

Implement recursive Merkle tree comparison to identify and transfer only differing entities. This is the primary state-based sync strategy.

## Protocol Flow

```
Initiator                          Responder
    │                                   │
    │ ──── TreeNodeRequest ───────────► │
    │      { node_id, depth }           │
    │                                   │
    │ ◄──── TreeNodeResponse ────────── │
    │      { nodes: [TreeNode] }        │
    │                                   │
    │ (Compare hashes, recurse on diff) │
    │                                   │
    │ ──── TreeNodeRequest ───────────► │
    │      { differing subtree }        │
    │                                   │
    │ ◄──── TreeNodeResponse ────────── │
    │      { leaf: TreeLeafData }       │
    │                                   │
    │ (CRDT merge entity)               │
    │                                   │
```

## Messages

```rust
pub struct TreeNodeRequest {
    pub node_id: [u8; 32],
    pub max_depth: Option<usize>,
}

pub struct TreeNodeResponse {
    pub nodes: Vec<TreeNode>,
}

pub struct TreeNode {
    pub id: [u8; 32],
    pub hash: [u8; 32],
    pub children: Vec<[u8; 32]>,
    pub leaf_data: Option<TreeLeafData>,
}

pub struct TreeLeafData {
    pub key: [u8; 32],
    pub value: Vec<u8>,
    pub metadata: Metadata,  // Includes crdt_type!
}
```

## Algorithm

1. Start at root
2. Request children of root
3. Compare child hashes with local
4. For each differing child:
   - If internal node: recurse
   - If leaf: request entity data
5. Apply received entities via CRDT merge

## Implementation Tasks

- [ ] Define TreeNodeRequest/Response messages
- [ ] Define TreeNode and TreeLeafData structs
- [ ] Implement tree traversal in SyncManager
- [ ] Implement hash comparison logic
- [ ] Fetch and include Metadata in leaf responses
- [ ] Call CRDT merge for received entities
- [ ] Handle missing nodes gracefully

## CRDT Merge on Receive

When leaf data is received, MUST use CRDT merge:

```rust
fn apply_leaf(leaf: TreeLeafData) {
    let local = storage.get(leaf.key);
    let merged = crdt_merge(local, leaf.value, leaf.metadata)?;
    storage.put(leaf.key, merged);
}
```

## Acceptance Criteria

- [ ] Can traverse remote tree
- [ ] Only differing entities are transferred
- [ ] Metadata (crdt_type) is included in transfer
- [ ] CRDT merge is used (not overwrite)
- [ ] Complexity: O(log n) round trips for localized changes
- [ ] Unit tests for tree comparison

## Files to Modify

- `crates/node/src/sync/tree_sync.rs`
- `crates/node/primitives/src/sync.rs`
- `crates/storage/src/interface.rs`

## POC Reference

See `handle_tree_node_request()` and `apply_entity_with_merge()` in POC branch.
