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
// Available to WASM apps as calimero_*
fn calimero_storage_read(key_ptr: u64, key_len: u32) -> u64;
fn calimero_storage_write(key_ptr: u64, key_len: u32, value_ptr: u64, value_len: u32);
```

### Context Functions (`logic/host_functions/context.rs`)

```rust
fn calimero_context_id() -> u64;
fn calimero_executor_id() -> u64;
```

## Patterns

### Adding a Host Function

- ✅ DO: Add to appropriate file in `src/logic/host_functions/`
- ✅ DO: Register in `src/logic/imports.rs`
- ✅ DO: Follow existing naming: `calimero_<category>_<action>`

```rust
// src/logic/host_functions/storage.rs
pub fn calimero_storage_read(
    mut env: FunctionEnvMut<'_, VMLogic>,
    key_ptr: u64,
    key_len: u32,
) -> VMResult<u64> {
    let logic = env.data_mut();
    // Implementation
}
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
# Find host functions
rg -n "pub fn calimero_" src/logic/host_functions/

# Find WASM imports registration
rg -n "namespace!" src/logic/imports.rs

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
