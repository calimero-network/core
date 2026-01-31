# CRITICAL AUDIT: Sync Protocol Implementation Gaps

**Date**: 2026-01-31  
**Branch**: `test/tree_sync`  
**Status**: 🔴 CRITICAL BUGS IDENTIFIED

---

## Executive Summary

The CIP claims Phase 2 (Hybrid Merge Architecture) is ✅ DONE, but **the merge registry is EMPTY in production**. All "built-in CRDT merge" is actually LWW fallback.

---

## Bug 1: CRDT Merge Registry Not Populated

### The Claim (CIP)

```
Phase 2: Hybrid Merge Architecture ✅ DONE
- Built-in CRDTs merge in storage layer (~100ns)
- Counter → sum per-node counts
- UnorderedMap → per-key merge  
- Custom types → WASM callback
```

### The Reality

```rust
// interface.rs - Counter merge
Some(CrdtType::Counter) => {
    // "For now, fallback to registry or LWW since Counter has complex internal structure"
    Self::try_merge_via_registry_or_lww(...)  // ← ALWAYS LWW!
}

// merge/registry.rs - try_merge_registered
pub fn try_merge_registered(...) -> Option<...> {
    let registry = MERGE_REGISTRY.read().ok()?;  // ← EMPTY IN PRODUCTION
    
    for entry in registry.values() {  // ← NEVER EXECUTES
        // ...
    }
}
```

**The registry is only populated in TESTS:**
```rust
// crates/storage/src/tests/merge_integration.rs
register_crdt_merge::<AppWithCounters>();  // ← TEST ONLY
```

**In production, NO merge functions are registered!**

### Impact

| CrdtType | CIP Claims | Actual Behavior |
|----------|------------|-----------------|
| Counter | Sum per-node counts | ❌ LWW (data loss) |
| UnorderedMap | Per-key merge | ❌ LWW on container |
| UnorderedSet | Add-wins union | ❌ LWW on container |
| Vector | Element-wise merge | ❌ LWW on container |
| Rga | Character merge | ❌ LWW on container |
| Custom | WASM callback | ❌ LWW (WASM not impl) |

---

## Bug 2: WASM Merge Callback Not Implemented

### The Claim

```rust
// crates/runtime/src/merge_callback.rs docs
RuntimeMergeCallback: Production implementation that calls into WASM
```

### The Reality

```rust
impl RuntimeMergeCallback {
    pub fn from_module(_module: &crate::Module) -> Option<Self> {
        // TODO: Check if module has __calimero_merge export
        // For now, return None to indicate WASM merge is not available
        None  // ← ALWAYS RETURNS NONE
    }
}

impl WasmMergeCallback for RuntimeMergeCallback {
    fn merge_custom(&self, type_name: &str, ...) {
        // NOTE: WASM merge not implemented - see method docs for limitations
        warn!("WASM merge not yet implemented, falling back to type registry or LWW");
        // Falls back to LWW
    }
}
```

### Impact

- Custom `#[derive(Mergeable)]` types: **LWW fallback**
- App-defined merge logic: **NEVER CALLED**
- CrdtType::Custom: **Always loses CRDT semantics**

---

## Bug 3: Collection Merge is LWW Not Entry-Level

### The Claim

```
Collections are merged at the entry level via their child IDs
```

### The Reality

```rust
Some(CrdtType::UnorderedMap)
| Some(CrdtType::UnorderedSet)
| Some(CrdtType::Vector) => {
    // "The collection container itself uses LWW for its metadata"
    let winner = if remote_metadata.updated_at() >= local_metadata.updated_at() {
        remote_data  // ← ENTIRE COLLECTION REPLACED!
    } else {
        local_data
    };
    Ok(Some(winner.to_vec()))
}
```

This is NOT entry-level merge. This replaces the ENTIRE collection based on timestamp.

### Impact

Node A adds key "foo", Node B adds key "bar" concurrently:
- **Expected**: Both keys preserved (entry-level merge)
- **Actual**: One node's entire map wins (LWW)

---

## Bug 4: TreeLeafData Metadata May Be Wrong

### The Issue

We now include metadata in `TreeLeafData`, but we read it from `Key::Index(id)`. This assumes:

1. The Index exists for the entity (may not for new entities)
2. The Index was written by the same WASM execution context (may not match)

### The Code

```rust
// manager.rs - handle_tree_node_request
let metadata = match store_handle.get(&index_state_key) {
    Ok(Some(index_value)) => {
        match borsh::from_slice::<EntityIndex>(index_value.as_ref()) {
            Ok(index) => index.metadata.clone(),
            Err(e) => Metadata::new(0, 0)  // ← DEFAULT LwwRegister
        }
    }
    _ => Metadata::new(0, 0)  // ← DEFAULT LwwRegister
};
```

If Index doesn't exist or can't be read, we default to `LwwRegister` even if the actual entity is a Counter!

---

## What Actually Works

| Feature | Status |
|---------|--------|
| Protocol Negotiation (SyncHandshake) | ✅ Works |
| TreeLeafData includes metadata | ✅ Works |
| Interface::merge_by_crdt_type_with_callback | ✅ Works (but dispatches to LWW) |
| Delta sync (DAG-based) | ✅ Works (goes through WASM) |
| Snapshot sync | ✅ Works (no merge needed) |
| Bloom filter sync | ⚠️ LWW only |
| Hash comparison sync | ⚠️ LWW only |

---

## Fixes Required

### Fix 1: Auto-Register Built-in CRDTs

```rust
// In storage crate initialization
lazy_static! {
    static ref _INIT: () = {
        register_crdt_merge::<Counter>();
        register_crdt_merge::<UnorderedMap<Vec<u8>, Vec<u8>>>();
        // etc.
    };
}
```

### Fix 2: Implement Proper Collection Merge

```rust
Some(CrdtType::UnorderedMap) => {
    // Deserialize both maps
    let local_map: HashMap<Key, Value> = deserialize(local_data)?;
    let remote_map: HashMap<Key, Value> = deserialize(remote_data)?;
    
    // Merge per-key with LWW per entry
    let mut merged = local_map.clone();
    for (k, v) in remote_map {
        merged.entry(k)
            .and_modify(|local_v| {
                // Per-entry LWW based on entry timestamps
            })
            .or_insert(v);
    }
    
    Ok(Some(serialize(&merged)?))
}
```

### Fix 3: Implement WASM Merge Callback

```rust
impl RuntimeMergeCallback {
    pub fn from_module(module: &crate::Module) -> Option<Self> {
        // Check for __calimero_merge export
        if module.has_export("__calimero_merge") {
            Some(Self { module: module.clone() })
        } else {
            None
        }
    }
}
```

### Fix 4: Proper Counter Merge

```rust
Some(CrdtType::Counter) => {
    let local_counter: Counter = deserialize(local_data)?;
    let remote_counter: Counter = deserialize(remote_data)?;
    
    // Sum per-node counts
    local_counter.merge(&remote_counter);
    
    Ok(Some(serialize(&local_counter)?))
}
```

---

## Timeline

| Priority | Fix | Effort |
|----------|-----|--------|
| P0 | Auto-register built-in CRDTs | 1 day |
| P0 | Proper Counter merge | 1 day |
| P1 | Proper Collection merge | 3 days |
| P2 | WASM merge callback | 1 week |

---

## Conclusion

**The benchmarks we ran were measuring LWW performance, not CRDT performance.**

The CIP claims are aspirational documentation, not implementation status. The actual merge path is:

```
crdt_type → try registry (empty) → LWW
```

Every single merge test in production is doing Last-Write-Wins.
