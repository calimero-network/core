# Issue 015: Snapshot Cryptographic Verification

**Priority**: P0 (Security Critical)  
**CIP Section**: §8 - Cryptographic Verification  
**Invariant**: I7 (Verification Before Apply)

## Summary

Implement cryptographic verification of snapshots BEFORE applying any data. This prevents accepting tampered state from malicious peers.

## Verification Steps

1. Receive all snapshot pages
2. Compute Merkle root from received entities
3. Compare computed root with claimed root
4. Only apply if match

## Verification Algorithm

```rust
impl Snapshot {
    pub fn verify(&self, claimed_root: [u8; 32]) -> Result<(), VerificationError> {
        // Build leaf hashes from entities
        let mut leaf_hashes: Vec<[u8; 32]> = self.entities
            .iter()
            .map(|e| hash_entity(&e.id, &e.data))
            .collect();
        
        // Sort for deterministic tree construction
        leaf_hashes.sort();
        
        // Build Merkle tree
        let computed_root = build_merkle_root(&leaf_hashes);
        
        if computed_root != claimed_root {
            return Err(VerificationError::RootHashMismatch {
                expected: claimed_root,
                computed: computed_root,
            });
        }
        
        Ok(())
    }
}

fn hash_entity(id: &[u8; 32], data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(id);
    hasher.update(data);
    hasher.finalize().into()
}

fn build_merkle_root(leaves: &[[u8; 32]]) -> [u8; 32] {
    if leaves.is_empty() {
        return [0u8; 32];
    }
    
    let mut level = leaves.to_vec();
    while level.len() > 1 {
        let mut next_level = Vec::new();
        for chunk in level.chunks(2) {
            let hash = if chunk.len() == 2 {
                hash_pair(&chunk[0], &chunk[1])
            } else {
                chunk[0]  // Odd element promoted
            };
            next_level.push(hash);
        }
        level = next_level;
    }
    level[0]
}
```

## Error Types

```rust
pub enum VerificationError {
    RootHashMismatch {
        expected: [u8; 32],
        computed: [u8; 32],
    },
    MissingEntities {
        count: usize,
    },
    CorruptedEntity {
        id: [u8; 32],
    },
}
```

## Usage in Sync

```rust
fn handle_snapshot_sync(
    pages: Vec<SnapshotPage>,
    complete: SnapshotComplete,
) -> Result<()> {
    // Assemble snapshot
    let snapshot = Snapshot::from_pages(pages)?;
    
    // VERIFY BEFORE APPLY (Invariant I7)
    snapshot.verify(complete.root_hash)?;
    
    // Now safe to apply
    apply_snapshot(snapshot)?;
    
    Ok(())
}
```

## Implementation Tasks

- [ ] Implement `Snapshot::verify()`
- [ ] Implement consistent entity hashing
- [ ] Implement Merkle tree construction
- [ ] Add verification before any apply
- [ ] Log verification failures with details
- [ ] Add metrics for verification time

## Security Considerations

- Verification MUST happen before ANY writes
- Verification failure MUST NOT modify state
- Log all verification failures (potential attacks)
- Consider rate limiting snapshot requests

## Acceptance Criteria

- [ ] Valid snapshot passes verification
- [ ] Tampered entity fails verification
- [ ] Tampered root hash fails verification
- [ ] No state modified on failure
- [ ] Verification time is logged
- [ ] Unit tests for all failure modes

## Files to Modify

- `crates/node/src/sync/snapshot_sync.rs`
- `crates/storage/src/interface.rs`

## POC Reference

See `Snapshot::verify()` implementation in POC branch.
