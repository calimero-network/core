# KV Store Application

A simple key-value store application built with Calimero SDK.

## Building

To build the application for WASM:

```bash
rustup target add wasm32-unknown-unknown
cargo build -p kv-store --target wasm32-unknown-unknown
```

## ABI Extraction

To extract the ABI from the compiled WASM:

```bash
calimero-abi extract target/wasm32-unknown-unknown/debug/kv_store.wasm -o apps/kv-store/res/abi.json
```

## State Schema

The state schema (state root type and all its dependencies) is automatically generated during build and written to `res/state-schema.json`. This schema contains only the types needed for state serialization/deserialization, making it useful for tools like `merodb` that need to decode state without loading the full WASM module.

### Build-time Generation

The state schema is automatically emitted during build:

```bash
cargo build -p kv-store --target wasm32-unknown-unknown
# res/state-schema.json is automatically created
```

### Extraction from WASM

You can also extract the state schema from a compiled WASM file:

```bash
# Build the calimero-abi tool
cargo build -p mero-abi

# Extract state schema from WASM
./target/debug/calimero-abi state apps/kv-store/res/kv-store.wasm -o state-schema.json
```

Both methods produce the same format, which can be used with `merodb`:

```bash
merodb export --db-path /path/to/db --all --state-schema-file res/state-schema.json --output export.json
```

## Canonical Types

The ABI uses the following canonical WASM types:

- **Scalar types**: `bool`, `i32`, `i64`, `u32`, `u64`, `f32`, `f64`, `string`, `bytes`
- **Collection types**: `list<T>`, `map<string,V>`
- **Nullable types**: `Option<T>` is represented as nullable `T`
- **Result types**: `Result<T,E>` is normalized to return `T` with errors handled separately

## Type Normalization Rules

- `usize`/`isize` → `u32`/`i32` (wasm32)
- `&str` → `string`
- `Vec<T>` → `list<T>`
- `Option<T>` → nullable `T`
- `Result<T,E>` → `T` (error handling separate)
- Custom types → `$ref` to type name (use `#[app::abi_type]` to expand)

## Type Expansion

By default, custom types are referenced using `$ref` in the ABI. To expand a type definition in the ABI, use the `#[app::abi_type]` macro:

```rust
#[app::abi_type]
pub struct MyCustomType {
    field1: String,
    field2: i32,
}
```

This will expand the type definition in the ABI's `types` section, making it available for reference by other types. 