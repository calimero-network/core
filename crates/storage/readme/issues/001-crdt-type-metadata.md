# Issue 001: Add CrdtType to Entity Metadata

**Priority**: P0 (Foundation)  
**CIP Section**: Appendix A - Hybrid Merge Architecture  
**Invariant**: I10 (Metadata Persistence)

## Summary

Add `crdt_type: Option<CrdtType>` to entity `Metadata` to enable proper CRDT merge dispatch during state synchronization.

## Motivation

Without knowing the CRDT type, state sync falls back to Last-Write-Wins (LWW), which causes **data loss** for concurrent updates on Counters, Maps, Sets, etc.

## Requirements

### CrdtType Enum

```rust
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug)]
pub enum CrdtType {
    // Built-in types (merge in storage layer)
    Counter,
    LwwRegister,
    Rga,
    UnorderedMap,
    UnorderedSet,
    Vector,
    
    // Custom types (require WASM callback)
    Custom { type_name: String },
}
```

### Updated Metadata

```rust
pub struct Metadata {
    pub created_at: u64,
    pub updated_at: UpdatedAt,
    pub storage_type: StorageType,
    pub crdt_type: Option<CrdtType>,  // NEW
    
    #[deprecated]
    pub resolution: ResolutionStrategy,
}
```

## Implementation Tasks

- [ ] Add `CrdtType` enum to `crates/storage/src/entities.rs`
- [ ] Add `crdt_type` field to `Metadata` struct
- [ ] Ensure Borsh serialization is backward compatible (Option<> handles missing field)
- [ ] Add helper methods: `Metadata::with_crdt_type()`, `Metadata::is_builtin_crdt()`
- [ ] Update `EntityIndex` to persist metadata changes

## Acceptance Criteria

- [ ] Existing data without `crdt_type` loads successfully (None)
- [ ] New entities can have `crdt_type` set
- [ ] Metadata persists across restarts
- [ ] Unit tests for serialization/deserialization

## Files to Modify

- `crates/storage/src/entities.rs`
- `crates/storage/src/index.rs`

## POC Reference

See POC Phase 2 in [POC-IMPLEMENTATION-NOTES.md](../POC-IMPLEMENTATION-NOTES.md)
