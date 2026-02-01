# Issue 002: Deterministic Entity/Collection IDs

**Priority**: P0 (Foundation)  
**CIP Section**: Protocol Invariants  
**Invariant**: I9 (Deterministic Entity IDs)

## Summary

Entity and collection IDs MUST be deterministic given the same application code and field names. Random IDs cause "ghost entities" that prevent proper CRDT merge.

## Problem

Currently, collection constructors use `Id::random()`:

```rust
// BAD: Random ID breaks sync
fn new() -> Self {
    let id = Id::random();  // Different on each node!
    // ...
}
```

This means:
- Node A: `items: UnorderedMap` → ID `0xABC...`
- Node B: `items: UnorderedMap` → ID `0xDEF...`

After sync, entries exist but are **orphaned** - the collection can't find them.

## Solution

Derive collection IDs from parent ID + field name hash:

```rust
fn new_with_field_name(parent_id: Option<Id>, field_name: &str) -> Self {
    let id = if let Some(parent) = parent_id {
        // Deterministic: hash(parent || field_name)
        let mut hasher = Sha256::new();
        hasher.update(parent.as_bytes());
        hasher.update(field_name.as_bytes());
        Id::new(hasher.finalize().into())
    } else {
        // Root-level: hash(field_name)
        let mut hasher = Sha256::new();
        hasher.update(field_name.as_bytes());
        Id::new(hasher.finalize().into())
    };
    // ...
}
```

## Implementation Tasks

- [ ] Add `new_with_field_name()` to all collection types:
  - [ ] `Counter`
  - [ ] `UnorderedMap`
  - [ ] `UnorderedSet`
  - [ ] `Vector`
  - [ ] `Rga`
  - [ ] `LwwRegister`
- [ ] Update `#[app::state]` macro to pass field names
- [ ] Deprecate `new()` that uses random IDs
- [ ] Add migration path for existing random IDs

## Acceptance Criteria

- [ ] Same code on two nodes produces identical collection IDs
- [ ] Nested collections derive IDs correctly (parent + field)
- [ ] Existing apps continue to work (backward compatibility)
- [ ] Unit tests verify determinism

## Files to Modify

- `crates/storage/src/collections/*.rs`
- `crates/sdk/macros/src/state.rs`

## POC Reference

See Bug 5 (Collection IDs random) in [POC-IMPLEMENTATION-NOTES.md](../POC-IMPLEMENTATION-NOTES.md)
