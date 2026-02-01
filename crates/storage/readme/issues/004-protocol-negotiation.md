# Issue 004: Protocol Negotiation & Selection

**Priority**: P0 (Core Protocol)  
**CIP Section**: §2.3 - Protocol Selection Rules  
**Depends On**: 003-sync-handshake-messages

## Summary

Implement the protocol selection algorithm that chooses the optimal sync strategy based on handshake information.

## Protocol Selection Decision Table

| # | Condition | Selected Protocol |
|---|-----------|-------------------|
| 1 | `local.root_hash == remote.root_hash` | `None` (already synced) |
| 2 | `!local.has_state` (fresh node) | `Snapshot` |
| 3 | `local.has_state` AND divergence > 50% | `HashComparison` |
| 4 | `max_depth > 3` AND divergence < 20% | `SubtreePrefetch` |
| 5 | `entity_count > 50` AND divergence < 10% | `BloomFilter` |
| 6 | `max_depth <= 2` AND many children | `LevelWise` |
| 7 | (default) | `HashComparison` |

## Critical Constraints

> **INVARIANT I5**: Snapshot MUST NOT be selected for initialized nodes.

```rust
fn select_protocol(local: &SyncHandshake, remote: &SyncHandshake) -> SyncProtocol {
    // Rule 1: Already synced
    if local.root_hash == remote.root_hash {
        return SyncProtocol::None;
    }
    
    // Rule 2: Fresh node - Snapshot allowed
    if !local.has_state {
        return SyncProtocol::Snapshot { ... };
    }
    
    // CRITICAL: Initialized node - NEVER use Snapshot
    // Rules 3-7 all use CRDT merge...
}
```

## Implementation Tasks

- [ ] Implement `select_protocol()` function
- [ ] Calculate divergence ratio: `|local.count - remote.count| / max(remote.count, 1)`
- [ ] Implement fallback logic when preferred protocol not supported
- [ ] Add logging for protocol selection decisions
- [ ] Handle version mismatches gracefully

## SyncProtocol Enum

```rust
pub enum SyncProtocol {
    None,
    DeltaSync { missing_delta_ids: Vec<[u8; 32]> },
    HashComparison { root_hash: [u8; 32], divergent_subtrees: Vec<[u8; 32]> },
    BloomFilter { filter_size: usize, false_positive_rate: f32 },
    SubtreePrefetch { subtree_roots: Vec<[u8; 32]> },
    LevelWise { max_depth: usize },
    Snapshot { compressed: bool, verified: bool },
}
```

## Acceptance Criteria

- [ ] Fresh node selects Snapshot
- [ ] Initialized node with >50% divergence selects HashComparison (NOT Snapshot)
- [ ] Protocol falls back gracefully when not mutually supported
- [ ] Decision is logged for debugging
- [ ] Unit tests for all decision paths

## Files to Modify

- `crates/node/src/sync/manager.rs`
- `crates/node/primitives/src/sync.rs`

## POC Reference

See Phase 4 (Integration) in [POC-IMPLEMENTATION-NOTES.md](../POC-IMPLEMENTATION-NOTES.md)
