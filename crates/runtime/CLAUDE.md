# calimero-runtime - WASM Execution

WebAssembly runtime using wasmer to compile and execute Calimero applications.

- **Crate**: `calimero-runtime`
- **Entry**: `src/lib.rs`
- **Frameworks**: wasmer (WASM), tokio (async)

## Build & Test

```bash
cargo build -p calimero-runtime
cargo test -p calimero-runtime
cargo test -p calimero-runtime test_storage -- --nocapture
cargo run -p calimero-runtime --example demo
cargo run -p calimero-runtime --example rps
```

## File Layout

```
src/
├── lib.rs                        # Calimero struct, public API
├── store.rs                      # WASM module compilation & caching
├── constraint.rs                 # Execution limits (time, memory)
├── constants.rs
├── memory.rs
├── errors.rs
├── panic_payload.rs
├── logic/
│   ├── imports.rs                # WASM import registration
│   ├── registers.rs              # Register management
│   ├── traits.rs
│   ├── errors.rs
│   └── host_functions/
│       ├── storage.rs            # storage_read, storage_write, storage_remove
│       ├── context.rs            # context_id, executor_id, context_create
│       ├── blobs.rs              # blob_create, blob_read
│       ├── utility.rs
│       ├── system.rs
│       ├── governance.rs
│       └── js_collections.rs
examples/
├── demo.rs
├── fetch.rs
└── rps.rs                        # Requests/sec benchmark
```

## Host Functions

Host functions are WASM imports callable from apps. They live in `src/logic/host_functions/` and are declared in `src/logic/imports.rs`.

### Naming Convention

`<category>_<action>` — e.g. `storage_read`, `context_id`, `blob_create`

### Adding a Host Function

1. Implement in the appropriate `src/logic/host_functions/<category>.rs`
2. Register in `src/logic/imports.rs` using the `imports!` macro

```rust
// src/logic/host_functions/storage.rs
pub fn storage_read(
    &mut self,
    src_key_ptr: u64,
    dest_register_id: u64,
) -> VMLogicResult<u32> {
    // use self.read_guest_memory_typed(...)
}

// src/logic/imports.rs — add declaration:
// fn storage_read(key_ptr: u64, register_id: u64) -> u32;
```

### VMLogic

```rust
pub struct VMLogic {
    registers: Registers,
    context:   RuntimeContext,
    storage:   Box<dyn Storage>,
}
```

## Key Files

| File | Purpose |
|---|---|
| `src/lib.rs` | `Calimero` struct, public API |
| `src/logic/imports.rs` | WASM import declarations |
| `src/logic/host_functions/storage.rs` | Storage operations |
| `src/logic/host_functions/context.rs` | Context operations |
| `src/store.rs` | Module compilation & caching |
| `src/constraint.rs` | Execution limits |

## Quick Search

```bash
rg -n "pub fn " src/logic/host_functions/
rg -n "fn " src/logic/imports.rs
rg -n "impl VMLogic" src/logic/
rg -n "pub enum" src/errors.rs
```

## Debugging

```bash
RUST_LOG=calimero_runtime=debug cargo run -p merod -- --node node1 run
```

## Gotchas

- Host functions use raw pointer args (`u64`) — be careful with memory safety
- Return values go through registers, not direct Rust returns
- WASM memory is isolated per instance
- Constraints cap execution time and memory per call
