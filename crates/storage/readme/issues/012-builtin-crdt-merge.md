# Issue 012: Built-in CRDT Merge in Storage Layer

**Priority**: P0 (Core)  
**CIP Section**: Appendix A - Hybrid Merge Architecture  
**Depends On**: 001-crdt-type-metadata

## Summary

Implement deterministic merge functions for built-in CRDTs in the storage layer, without requiring WASM.

## Supported Types

| Type | Merge Strategy |
|------|----------------|
| Counter | Sum per-node counts |
| UnorderedMap | Per-key merge (recursive) |
| UnorderedSet | Add-wins union |
| Vector | Element-wise merge |
| Rga | Tombstone-based merge |
| LwwRegister | Timestamp comparison |

## Merge Dispatch Function

```rust
pub fn merge_by_crdt_type(
    local: &[u8],
    remote: &[u8],
    metadata: &Metadata,
) -> Result<Vec<u8>, MergeError> {
    match &metadata.crdt_type {
        Some(CrdtType::Counter) => merge_counter(local, remote),
        Some(CrdtType::UnorderedMap) => merge_map(local, remote),
        Some(CrdtType::UnorderedSet) => merge_set(local, remote),
        Some(CrdtType::Vector) => merge_vector(local, remote),
        Some(CrdtType::Rga) => merge_rga(local, remote),
        Some(CrdtType::LwwRegister) => merge_lww(local, remote),
        Some(CrdtType::Custom { .. }) => Err(MergeError::WasmRequired),
        None => merge_lww_fallback(local, remote, metadata),
    }
}
```

## Implementation Tasks

### Counter Merge
- [ ] Deserialize both counters
- [ ] Sum per-node counts (G-Counter semantics)
- [ ] Serialize result

### UnorderedMap Merge
- [ ] Deserialize both maps
- [ ] For each key: merge values recursively
- [ ] Handle keys only in one map (add)
- [ ] Serialize result

### UnorderedSet Merge
- [ ] Deserialize both sets
- [ ] Union (add-wins)
- [ ] Serialize result

### Vector Merge
- [ ] Deserialize both vectors
- [ ] Element-wise merge (same index = LWW)
- [ ] Handle different lengths
- [ ] Serialize result

### Rga Merge
- [ ] Deserialize both RGAs
- [ ] Merge tombstones
- [ ] Preserve all insertions
- [ ] Serialize result

### LwwRegister Merge
- [ ] Compare HLC timestamps
- [ ] Higher timestamp wins
- [ ] Tie-breaker: lexicographic on data

### LWW Fallback
- [ ] Used when `crdt_type` is None
- [ ] **Log warning** - indicates missing type info
- [ ] Compare timestamps, remote wins on tie

## Error Handling

```rust
pub enum MergeError {
    CrdtMergeError(String),
    WasmRequired { type_name: String },
    SerializationError(String),
    TypeMismatch { expected: String, found: String },
}
```

## Acceptance Criteria

- [ ] Counter merge sums correctly
- [ ] Map merge preserves all keys
- [ ] Set merge is add-wins
- [ ] LWW uses HLC correctly
- [ ] Fallback logs warning
- [ ] All merges are deterministic
- [ ] Unit tests for each type

## Files to Modify

- `crates/storage/src/interface.rs`
- `crates/storage/src/collections/*.rs`

## POC Reference

See `merge_by_crdt_type_with_callback()` in `crates/storage/src/interface.rs`
