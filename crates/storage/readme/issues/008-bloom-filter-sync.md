# Issue 008: BloomFilter Sync Strategy

**Priority**: P1  
**CIP Section**: Appendix B - Protocol Selection Matrix  
**Depends On**: 007-hash-comparison-sync

## Summary

Implement Bloom filter-based sync for large trees with small divergence (<10%). Provides O(1) diff detection with configurable false positive rate.

## When to Use

- `entity_count > 50`
- `divergence < 10%`
- Want to minimize round trips

## Protocol Flow

```
Initiator                          Responder
    │                                   │
    │ ──── BloomFilterRequest ────────► │
    │      { filter, fp_rate }          │
    │                                   │
    │ ◄──── BloomFilterResponse ─────── │
    │      { missing_entities: [...] }  │
    │                                   │
    │ (CRDT merge entities)             │
    │                                   │
```

## Bloom Filter Implementation

```rust
pub struct DeltaIdBloomFilter {
    bits: Vec<u8>,
    num_bits: usize,
    num_hashes: u8,
}

impl DeltaIdBloomFilter {
    /// Use consistent hash function (FNV-1a)
    pub fn hash_fnv1a(data: &[u8]) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in data {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }
    
    pub fn insert(&mut self, id: &[u8; 32]) { ... }
    pub fn contains(&self, id: &[u8; 32]) -> bool { ... }
}
```

## Messages

```rust
pub struct BloomFilterRequest {
    pub filter: DeltaIdBloomFilter,
    pub false_positive_rate: f32,
}

pub struct BloomFilterResponse {
    pub missing_entities: Vec<TreeLeafData>,
}
```

## Implementation Tasks

- [ ] Implement `DeltaIdBloomFilter` with consistent FNV-1a hash
- [ ] Build filter from local entity IDs
- [ ] Responder: scan entities not in filter
- [ ] Return missing entities with metadata
- [ ] Apply via CRDT merge
- [ ] Tune filter size for target FP rate

## Critical: Consistent Hash Function

Both nodes MUST use the same hash function. POC bug: one used SipHash, other used FNV-1a.

```rust
// CORRECT: Both use FNV-1a
let hash = DeltaIdBloomFilter::hash_fnv1a(&entity_id);

// WRONG: Different hash functions
// let hash = DefaultHasher::new().write(&entity_id);  // SipHash!
```

## Acceptance Criteria

- [ ] Filter correctly identifies missing entities
- [ ] False positive rate matches configuration
- [ ] Hash function is consistent across nodes
- [ ] Missing entities include metadata for CRDT merge
- [ ] Complexity: O(n) scan, but only 1-2 round trips

## Files to Modify

- `crates/node/primitives/src/sync_protocol.rs`
- `crates/node/src/sync/bloom_sync.rs` (new)
- `crates/dag/src/lib.rs` (if used for deltas)

## POC Reference

See Bug 5 (Bloom filter hash mismatch) in [POC-IMPLEMENTATION-NOTES.md](../POC-IMPLEMENTATION-NOTES.md)
