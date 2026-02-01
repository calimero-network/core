# Issue 013: WASM Merge Callback for Custom Types

**Priority**: P1  
**CIP Section**: Appendix A - WASM Merge Callback Interface  
**Depends On**: 012-builtin-crdt-merge

## Summary

Implement WASM callback interface for merging custom `Mergeable` types defined by applications.

## When to Use

- `crdt_type == Custom { type_name }`
- Application defines `impl Mergeable for MyType`
- Cannot merge in storage layer alone

## Callback Interface

```rust
/// Trait for WASM merge callback
pub trait WasmMergeCallback: Send + Sync {
    /// Merge custom type via WASM
    fn merge(
        &self,
        local: &[u8],
        remote: &[u8],
        type_name: &str,
    ) -> Result<Vec<u8>, MergeError>;
    
    /// Merge root state (always custom)
    fn merge_root_state(
        &self,
        local: &[u8],
        remote: &[u8],
    ) -> Result<Vec<u8>, MergeError>;
}
```

## WASM Module Export

Applications must export merge functions:

```rust
// In application WASM
#[no_mangle]
pub extern "C" fn __calimero_merge(
    local_ptr: *const u8,
    local_len: usize,
    remote_ptr: *const u8,
    remote_len: usize,
    type_name_ptr: *const u8,
    type_name_len: usize,
) -> *mut MergeResult { ... }

#[no_mangle]
pub extern "C" fn __calimero_merge_root_state(
    local_ptr: *const u8,
    local_len: usize,
    remote_ptr: *const u8,
    remote_len: usize,
) -> *mut MergeResult { ... }
```

## Runtime Implementation

```rust
pub struct RuntimeMergeCallback {
    module: WasmModule,
}

impl WasmMergeCallback for RuntimeMergeCallback {
    fn merge(&self, local: &[u8], remote: &[u8], type_name: &str) -> Result<Vec<u8>> {
        // Call WASM export __calimero_merge
        self.module.call("__calimero_merge", local, remote, type_name)
    }
    
    fn merge_root_state(&self, local: &[u8], remote: &[u8]) -> Result<Vec<u8>> {
        // Call WASM export __calimero_merge_root_state
        self.module.call("__calimero_merge_root_state", local, remote)
    }
}

impl RuntimeMergeCallback {
    /// Create callback from loaded module (if exports exist)
    pub fn from_module(module: &WasmModule) -> Option<Self> {
        if module.has_export("__calimero_merge") {
            Some(Self { module: module.clone() })
        } else {
            None
        }
    }
}
```

## Integration with Sync

```rust
// In SyncManager
let wasm_callback = RuntimeMergeCallback::from_module(&self.wasm_module);

let merged = Interface::merge_by_crdt_type_with_callback(
    local_data,
    remote_data,
    &metadata,
    wasm_callback.as_ref(),
)?;
```

## Implementation Tasks

- [ ] Define `WasmMergeCallback` trait
- [ ] Define WASM export signatures
- [ ] Implement `RuntimeMergeCallback`
- [ ] Update SDK to generate merge exports
- [ ] Handle missing export gracefully (error)
- [ ] Add timeout for WASM calls

## SDK Macro Support

The `#[app::state]` macro should generate merge exports:

```rust
#[app::state]
struct MyApp {
    game: MyGameState,  // impl Mergeable
}

// Generated:
#[no_mangle]
pub extern "C" fn __calimero_merge_root_state(...) {
    let local: MyApp = deserialize(local)?;
    let remote: MyApp = deserialize(remote)?;
    local.merge(&remote)?;
    serialize(&local)
}
```

## Acceptance Criteria

- [ ] Custom types dispatch to WASM
- [ ] Root state merges via callback
- [ ] Missing export returns clear error
- [ ] Timeout prevents infinite WASM calls
- [ ] SDK generates required exports
- [ ] Unit tests for callback dispatch

## Files to Modify

- `crates/storage/src/interface.rs`
- `crates/runtime/src/lib.rs`
- `crates/sdk/macros/src/state.rs`

## POC Reference

See `WasmMergeCallback` trait and `RuntimeMergeCallback::from_module()` in POC branch.
