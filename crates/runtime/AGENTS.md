# calimero-runtime - WASM Execution

WebAssembly runtime using wasmer to compile and execute Calimero applications.

## Package Identity

- **Crate**: `calimero-runtime`
- **Entry**: `src/lib.rs`
- **Framework**: wasmer (WASM), tokio (async)

## Commands

```bash
# Build
cargo build -p calimero-runtime

# Test
cargo test -p calimero-runtime

# Run examples
cargo run -p calimero-runtime --example demo
cargo run -p calimero-runtime --example rps
```

## File Organization

```
src/
├── lib.rs                    # Public API, Calimero struct
├── store.rs                  # WASM module store
├── constraint.rs             # Execution constraints
├── constants.rs              # Runtime constants
├── memory.rs                 # Memory management
├── errors.rs                 # Error types
├── panic_payload.rs          # Panic handling
├── logic.rs                  # Logic module parent
├── logic/
│   ├── imports.rs            # WASM imports
│   ├── registers.rs          # Register management
│   ├── traits.rs             # Runtime traits
│   ├── errors.rs             # Logic errors
│   ├── host_functions.rs     # Host functions parent
│   └── host_functions/
│       ├── storage.rs        # Storage host functions
│       ├── context.rs        # Context host functions
│       ├── blobs.rs          # Blob host functions
│       ├── utility.rs        # Utility functions
│       ├── system.rs         # System functions
│       ├── governance.rs     # Governance functions
│       └── js_collections.rs # JS collections support
└── tests/
    └── errors.rs             # Error tests
examples/
├── demo.rs                   # Basic demo
├── fetch.rs                  # Fetch example
└── rps.rs                    # Requests per second benchmark
```

## Host Functions

Host functions are WASM imports callable from applications:

### Storage Functions (`logic/host_functions/storage.rs`)

```rust
// Registered in imports.rs, callable from WASM apps
fn storage_read(key_ptr: u64, register_id: u64) -> u32;
fn storage_write(key_ptr: u64, value_ptr: u64, register_id: u64) -> u32;
fn storage_remove(key_ptr: u64, register_id: u64) -> u32;
```

### Context Functions (`logic/host_functions/context.rs`)

```rust
fn context_id(register_id: u64);
fn executor_id(register_id: u64);
fn context_create(protocol_ptr: u64, app_id_ptr: u64, args_ptr: u64, alias_ptr: u64);
fn context_delete(context_id_ptr: u64);
```

## Patterns

### Adding a Host Function

- ✅ DO: Add to appropriate file in `src/logic/host_functions/`
- ✅ DO: Register in `src/logic/imports.rs` using the `imports!` macro
- ✅ DO: Follow existing naming: `<category>_<action>` (e.g. `storage_read`, `context_id`, `blob_create`)

```rust
// src/logic/host_functions/storage.rs
pub fn storage_read(
    &mut self,
    src_key_ptr: u64,
    dest_register_id: u64,
) -> VMLogicResult<u32> {
    // Implementation using self.read_guest_memory_typed, etc.
}

// Then register in src/logic/imports.rs:
// fn storage_read(key_ptr: u64, register_id: u64) -> u32;
```

### VMLogic Pattern

```rust
// Logic context for host functions
pub struct VMLogic {
    registers: Registers,
    context: RuntimeContext,
    storage: Box<dyn Storage>,
}
```

## Key Files

| File                                  | Purpose                      |
| ------------------------------------- | ---------------------------- |
| `src/lib.rs`                          | Calimero struct, public API  |
| `src/logic/imports.rs`                | WASM import registration     |
| `src/logic/host_functions/storage.rs` | Storage operations           |
| `src/logic/host_functions/context.rs` | Context operations           |
| `src/store.rs`                        | Module compilation & caching |
| `src/constraint.rs`                   | Execution limits             |

## JIT Index

```bash
# Find host functions (implementations)
rg -n "pub fn " src/logic/host_functions/

# Find host functions (WASM import declarations)
rg -n "fn " src/logic/imports.rs

# Find VMLogic methods
rg -n "impl VMLogic" src/logic/

# Find error types
rg -n "pub enum" src/errors.rs
```

## Debugging

```bash
# Enable WASM tracing
RUST_LOG=calimero_runtime=debug cargo run -p merod -- --node node1 run

# Test specific function
cargo test -p calimero-runtime test_storage -- --nocapture
```

## Common Gotchas

- Host functions use raw pointers (u64) - careful with memory safety
- Registers are used for return values (not direct returns)
- WASM memory is isolated per instance
- Constraints limit execution time and memory
