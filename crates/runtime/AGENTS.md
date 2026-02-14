# calimero-runtime - WASM Execution

WebAssembly runtime using wasmer to compile and execute Calimero applications.

## Package Identity

- **Crate**: `calimero-runtime`
- **Entry**: `src/lib.rs`
- **Framework**: wasmer (WASM), tokio (async)
- **Related Docs**: [README.md](README.md) (architecture), [HOST_FUNCTIONS.md](HOST_FUNCTIONS.md) (API reference)

## Quick Understanding

### What This Crate Does

1. **Compiles** WASM modules using Wasmer's Cranelift backend
2. **Executes** WASM functions with configurable resource limits
3. **Provides host functions** that bridge guest code to storage, events, context management
4. **Manages memory** between WASM guest and host via pointer-based buffer descriptors

### Key Architectural Insight

```
┌─────────────────────────────────────────────────────────────────┐
│                        RUNTIME EXECUTION                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│   Module::run()                                                 │
│       │                                                         │
│       ▼                                                         │
│   VMLogic (execution context)                                   │
│       │                                                         │
│       ├── registers: Registers (temporary data slots)           │
│       ├── storage: &mut dyn Storage (key-value persistence)     │
│       ├── limits: &VMLimits (resource constraints)              │
│       ├── logs: Vec<String>                                     │
│       ├── events: Vec<Event>                                    │
│       └── context_mutations: Vec<ContextMutation>               │
│                                                                 │
│   VMHostFunctions (self-referencing wrapper)                    │
│       │                                                         │
│       └── Provides all host functions to WASM imports           │
│           via read_guest_memory_* / write patterns              │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### Memory Exchange Pattern

All data passes between guest ↔ host via **buffer descriptors**:

```rust
// Guest side: create descriptor in memory
struct Buffer { ptr: u64, len: u64 }  // 16 bytes total

// Host side: read descriptor, then read actual data
let buf = self.read_guest_memory_typed::<sys::Buffer<'_>>(ptr)?;
let data = self.read_guest_memory_slice(&buf)?;
```

## Commands

```bash
# Build
cargo build -p calimero-runtime

# Test
cargo test -p calimero-runtime

# Test with traces
RUST_LOG=runtime=trace cargo test -p calimero-runtime -- --nocapture

# Run examples
cargo run -p calimero-runtime --example demo
cargo run -p calimero-runtime --example rps
```

## File Organization

```
src/
├── lib.rs                    # Public API: Engine, Module, run()
├── store.rs                  # Storage trait and InMemoryStorage
├── constraint.rs             # Execution constraints
├── constants.rs              # Runtime constants (DIGEST_SIZE = 32)
├── memory.rs                 # WasmerTunables for memory limits
├── errors.rs                 # Error types (HostError, VMRuntimeError, etc.)
├── panic_payload.rs          # Panic handling utilities
├── merge_callback.rs         # WASM merge callback for custom CRDT types
├── logic.rs                  # VMLogic, VMContext, VMLimits, VMHostFunctions
├── logic/
│   ├── imports.rs            # imports! macro registering all host functions
│   ├── registers.rs          # Register management
│   ├── traits.rs             # ContextHost trait
│   ├── errors.rs             # VMLogicError
│   └── host_functions/       # Host function implementations
│       ├── storage.rs        # storage_read/write/remove, private_storage_*
│       ├── context.rs        # context_create/delete/add_member/etc
│       ├── blobs.rs          # blob_create/write/close/open/read
│       ├── utility.rs        # fetch, random_bytes, time_now, ed25519_verify
│       ├── system.rs         # panic, registers, input/output, emit, commit
│       ├── governance.rs     # send_proposal, approve_proposal
│       └── js_collections.rs # js_crdt_* functions for JS SDK
└── tests/
    └── errors.rs             # Error handling tests
examples/
├── demo.rs                   # Basic key-value storage demo
├── fetch.rs                  # HTTP fetch example
└── rps.rs                    # Requests per second benchmark
```

## Key Concepts for AI Agents

### 1. Host Function Registration

Host functions are declared in `imports.rs` using the `imports!` macro:

```rust
// src/logic/imports.rs
imports! {
    fn panic(location_ptr: u64);
    fn storage_read(key_ptr: u64, register_id: u64) -> u32;
    // ... more functions
}
```

Implementation lives in `host_functions/*.rs`:

```rust
// src/logic/host_functions/storage.rs
impl VMHostFunctions<'_> {
    pub fn storage_read(&mut self, src_key_ptr: u64, dest_register_id: u64) -> VMLogicResult<u32> {
        let key = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_key_ptr)? };
        // ... implementation
    }
}
```

### 2. Adding a New Host Function

1. Add declaration to `src/logic/imports.rs`
2. Implement in appropriate `src/logic/host_functions/*.rs` file
3. Follow naming convention: `<category>_<action>` (e.g., `storage_read`)
4. Use `read_guest_memory_typed`, `read_guest_memory_slice` for input
5. Use `registers.set()` to return data to guest
6. Return status codes: `0` = not found, `1` = success

### 3. VMLogic Lifecycle

```rust
// 1. Create context
let context = VMContext::new(input, context_id, executor_id);

// 2. Create logic with storage and limits
let mut logic = VMLogic::new(storage, private_storage, context, limits, node_client, context_host);

// 3. Set up memory (from Wasmer instance)
logic.with_memory(memory);

// 4. Get host functions wrapper
let host = logic.host_functions(store_mut);

// 5. Execute WASM, host functions called via wrapper

// 6. Finish and get outcome
let outcome = logic.finish();
```

### 4. Self-Referencing Pattern

`VMHostFunctions` uses `ouroboros` crate's `#[self_referencing]` for safe self-referential borrows:

```rust
#[self_referencing]
pub struct VMHostFunctions<'a> {
    logic: &'a mut VMLogic<'a>,
    #[borrows(logic)]
    #[covariant]
    memory: &'this Memory,
}
```

This allows the struct to hold both a mutable reference to logic and a reference derived from it.

### 5. WASM Merge Callback for Custom Types

During sync, storage may need to merge custom CRDT types. Since Wasmer doesn't support reentrancy (calling WASM from within WASM), the runtime creates a **separate WASM instance** for merge callbacks:

```
Module::run()
  └── create_merge_callback() → RuntimeMergeCallback (separate instance)
  └── logic.with_merge_callback(callback)
      ...
      └── Host function triggers storage write
          └── Storage detects conflict with CrdtType::Custom
              └── Calls merge callback → __calimero_merge_{TypeName}
```

**Key files:**
- `src/merge_callback.rs` - `RuntimeMergeCallback` implementation
- `src/lib.rs` - `Module::create_merge_callback()` 
- `src/logic.rs` - `VMLogic.merge_callback` field

**WASM exports required for custom merge:**
- `__calimero_alloc(size) -> ptr` - Memory allocation
- `__calimero_merge_{TypeName}(local_ptr, local_len, remote_ptr, remote_len) -> result_ptr`

The `#[app::mergeable]` macro in calimero-sdk auto-generates these exports.

## JIT Index by Category

### Finding Function Declarations

```bash
# All host function declarations (WASM imports)
rg "^            fn [a-z_]+\(" src/logic/imports.rs

# Storage functions
rg "fn (storage|private_storage)_" src/logic/imports.rs

# Context functions
rg "fn context_" src/logic/imports.rs

# JS CRDT functions
rg "fn js_crdt_" src/logic/imports.rs
```

### Finding Implementations

```bash
# All host function implementations
rg "pub fn " src/logic/host_functions/

# Specific category implementations
rg "pub fn storage_" src/logic/host_functions/storage.rs
rg "pub fn context_" src/logic/host_functions/context.rs
rg "pub fn blob_" src/logic/host_functions/blobs.rs

# Memory access patterns
rg "read_guest_memory_typed" src/logic/host_functions/
rg "read_guest_memory_slice" src/logic/host_functions/
```

### Finding Types and Traits

```bash
# VMLogic definition
rg "pub struct VMLogic" src/logic.rs

# VMLimits configuration
rg "pub struct VMLimits" src/logic.rs

# Storage trait
rg "pub trait Storage" src/store.rs

# Error types
rg "pub enum (HostError|VMRuntimeError|VMLogicError)" src/
```

### Finding Tests

```bash
# Host function tests
rg "#\[test\]" src/logic/host_functions/

# Storage tests
rg "fn test_storage" src/logic/host_functions/storage.rs

# System tests
rg "fn test_" src/logic/host_functions/system.rs
```

## Common Patterns

### Reading Data from Guest

```rust
// 1. Read buffer descriptor
let buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(ptr)? };

// 2. Validate limits
if buf.len() > self.borrow_logic().limits.max_storage_key_size.get() {
    return Err(HostError::KeyLengthOverflow.into());
}

// 3. Read actual bytes
let data = self.read_guest_memory_slice(&buf)?.to_vec();
```

### Returning Data to Guest

```rust
// Write to register, guest can read via read_register
self.with_logic_mut(|logic| {
    logic.registers.set(logic.limits, register_id, data)
})?;
```

### Mutating State

```rust
// Use with_logic_mut for mutable access
self.with_logic_mut(|logic| {
    logic.logs.push(message);
    logic.events.push(event);
});
```

## Debugging

```bash
# Enable runtime tracing
RUST_LOG=calimero_runtime=debug cargo run -p merod -- --node node1 run

# Enable host function traces
RUST_LOG=runtime::host=trace cargo test -p calimero-runtime

# Enable guest log traces
RUST_LOG=runtime::guest=trace cargo test -p calimero-runtime

# Test specific function
cargo test -p calimero-runtime test_storage -- --nocapture
```

## Common Gotchas

1. **Host functions use raw pointers (u64)** - All memory access is through typed reads
2. **Registers are host-side** - Guest writes pointer, host stores data, guest reads back via `read_register`
3. **WASM memory is isolated** - Each instance has its own linear memory
4. **Constraints limit everything** - VMLimits enforced on keys, values, logs, events, registers
5. **Self-referencing pattern** - `VMHostFunctions` requires careful lifetime management
6. **JS CRDT functions are different** - They work with collection IDs, not raw storage keys
7. **Private storage is optional** - May be `None` in tests or minimal setups
8. **Context mutations are queued** - They don't happen immediately, applied after execution

## Related Crates

| Crate | Relationship |
|-------|--------------|
| `calimero-sys` | Defines `sys::Buffer`, `sys::Event`, etc. used by host functions |
| `calimero-storage` | CRDT collections, Merkle tree, used by `persist_root_state` |
| `calimero-primitives` | Common types like `ContextId`, `PublicKey` |
| `calimero-node` | Uses runtime to execute WASM in context execution |
