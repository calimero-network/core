# Issue 011: Snapshot Sync (Fresh Nodes Only)

**Priority**: P1  
**CIP Section**: §6 - Snapshot Sync Constraints  
**Invariant**: I5 (No Silent Data Loss), I7 (Verification Before Apply)

## Summary

Implement full state snapshot transfer for fresh node bootstrap. **CRITICAL**: This is ONLY for nodes with no existing state.

## When to Use

- `!local.has_state` (fresh node)
- Fastest way to bootstrap
- Verification REQUIRED before apply

## Protocol Flow

```
Initiator (Fresh)                  Responder
    │                                   │
    │ ──── SnapshotRequest ───────────► │
    │      { compressed: true }         │
    │                                   │
    │ ◄──── SnapshotPage ─────────────  │
    │      { page 1 of N }              │
    │                                   │
    │ ◄──── SnapshotPage ─────────────  │
    │      { page 2 of N }              │
    │                                   │
    │ ◄──── SnapshotComplete ────────── │
    │      { root_hash, total }         │
    │                                   │
    │ (Verify root hash)                │
    │ (Direct apply - no merge)         │
    │                                   │
```

## Messages

```rust
pub struct SnapshotRequest {
    pub compressed: bool,
}

pub struct SnapshotPage {
    pub page_number: usize,
    pub total_pages: usize,
    pub entities: Vec<SnapshotEntity>,
}

pub struct SnapshotEntity {
    pub id: [u8; 32],
    pub data: Vec<u8>,
    pub metadata: Metadata,
}

pub struct SnapshotComplete {
    pub root_hash: [u8; 32],
    pub total_entities: usize,
}
```

## Verification (Invariant I7)

Before applying ANY entity:

```rust
fn verify_snapshot(pages: &[SnapshotPage], claimed_root: [u8; 32]) -> Result<()> {
    // Rebuild Merkle tree from entities
    let computed_root = compute_root_from_entities(pages)?;
    
    if computed_root != claimed_root {
        return Err(VerificationError::RootHashMismatch);
    }
    Ok(())
}
```

## Safety Check (Invariant I5)

```rust
fn apply_snapshot(snapshot: Snapshot) -> Result<()> {
    // CRITICAL: Only for fresh nodes!
    if storage.has_state() {
        return Err(SyncError::SnapshotOnInitializedNode);
    }
    
    // Safe to directly apply (no CRDT merge needed)
    for entity in snapshot.entities {
        storage.put(entity.id, entity.data)?;
    }
    Ok(())
}
```

## Implementation Tasks

- [ ] Define Snapshot messages
- [ ] Implement paginated transfer
- [ ] Implement compression (zstd)
- [ ] Verify root hash before apply
- [ ] **BLOCK snapshot on initialized nodes**
- [ ] Create checkpoint delta after apply
- [ ] Handle transfer interruption

## Acceptance Criteria

- [ ] Fresh node can bootstrap via snapshot
- [ ] Verification fails on tampered data
- [ ] Initialized node REJECTS snapshot
- [ ] Compression reduces transfer size
- [ ] Pagination handles large state
- [ ] Checkpoint delta created after apply

## Files to Modify

- `crates/node/src/sync/snapshot_sync.rs` (new)
- `crates/node/primitives/src/sync.rs`
- `crates/storage/src/interface.rs`

## POC Reference

See snapshot handling and verification in POC branch.
