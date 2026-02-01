# Issue 014: Entity Transfer with Metadata (TreeLeafData)

**Priority**: P0 (Critical for CRDT merge)  
**CIP Section**: §7 - Wire Protocol  
**Invariant**: I10 (Metadata Persistence)  
**Depends On**: 001, 007

## Summary

Ensure all state-based sync strategies transfer entity metadata (including `crdt_type`) alongside entity data.

## Problem

If we transfer entity data without metadata, the receiver cannot dispatch CRDT merge correctly and falls back to LWW (data loss!).

## TreeLeafData Structure

```rust
/// Leaf entity data including metadata for CRDT merge
pub struct TreeLeafData {
    /// Entity key (32 bytes)
    pub key: [u8; 32],
    
    /// Entity value (serialized data)
    pub value: Vec<u8>,
    
    /// Entity metadata including crdt_type
    pub metadata: Metadata,
}
```

## All Strategies Must Include Metadata

| Strategy | Response Type | Must Include |
|----------|---------------|--------------|
| HashComparison | `TreeNodeResponse` | `leaf_data: Option<TreeLeafData>` |
| BloomFilter | `BloomFilterResponse` | `missing_entities: Vec<TreeLeafData>` |
| SubtreePrefetch | `SubtreePrefetchResponse` | `entities: Vec<TreeLeafData>` |
| LevelWise | `LevelWiseResponse` | `leaf_data: Option<TreeLeafData>` |
| Snapshot | `SnapshotPage` | `metadata: Metadata` in each entity |

## Implementation Tasks

- [ ] Define `TreeLeafData` struct
- [ ] Update `TreeNodeResponse` to use `TreeLeafData`
- [ ] Update `BloomFilterResponse` to use `TreeLeafData`
- [ ] Update `SubtreePrefetchResponse` to use `TreeLeafData`
- [ ] Update `LevelWiseResponse` to use `TreeLeafData`
- [ ] Update `SnapshotEntity` to include `Metadata`
- [ ] Fetch metadata from `EntityIndex` when building responses
- [ ] Persist metadata after applying received entities

## Request Handler

```rust
fn handle_tree_node_request(request: TreeNodeRequest) -> TreeNodeResponse {
    let node = storage.get_node(request.node_id)?;
    
    if node.is_leaf() {
        let entity = storage.get_entity(node.entity_id)?;
        let metadata = storage.get_metadata(node.entity_id)?;  // Include!
        
        TreeNodeResponse {
            nodes: vec![TreeNode {
                leaf_data: Some(TreeLeafData {
                    key: node.entity_id,
                    value: entity,
                    metadata,  // CRITICAL
                }),
                // ...
            }],
        }
    } else {
        // ... internal node handling
    }
}
```

## Apply Handler

```rust
fn apply_leaf_from_tree_data(leaf: TreeLeafData) -> Result<()> {
    // Merge using the metadata from the sender
    let local = storage.get(leaf.key);
    let merged = crdt_merge(local, &leaf.value, &leaf.metadata)?;
    
    // Store BOTH data and metadata
    storage.put(leaf.key, merged)?;
    storage.put_metadata(leaf.key, leaf.metadata)?;  // Persist!
    
    Ok(())
}
```

## Acceptance Criteria

- [ ] All strategies include metadata in transfer
- [ ] `crdt_type` is preserved across sync
- [ ] CRDT merge works correctly on receiver
- [ ] Metadata persists to storage
- [ ] Unit tests verify metadata flow

## Files to Modify

- `crates/node/primitives/src/sync.rs`
- `crates/node/src/sync/tree_sync.rs`
- `crates/node/src/sync/bloom_sync.rs`
- `crates/storage/src/index.rs`

## POC Reference

See Bug 6 (Metadata not persisted) and `TreeLeafData` in POC branch.
