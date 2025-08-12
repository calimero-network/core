# ABI Core

Core types and serialization for Calimero ABI (Application Binary Interface) generation.

## Overview

This crate provides the foundational types and serialization logic for generating ABIs from Rust code. It's designed to work with the `abi-macros` crate to automatically generate ABI definitions from annotated Rust modules.

## Features

- **Canonical JSON Serialization**: Deterministic JSON output for ABI files
- **SHA256 Hashing**: Cryptographic hashing of ABI definitions
- **Type System**: Comprehensive type definitions for ABI parameters and return values
- **Error Handling**: Support for Result<T,E> returns with structured error information
- **Cross-Platform**: No absolute paths or platform-specific separators in output

## Schema Version 0.1.1

The ABI schema has been updated to replace Rust-centric `Result<T,E>` returns with a universal contract structure:

### Function Shape

Functions now have the following structure:
- `returns`: `<TypeRef | null>` (success payload, `null` for unit type `()`)
- `errors`: `[{ name, code, type?<TypeRef> }]` derived from enum `E`

### Error Structure

```rust
pub struct ErrorAbi {
    pub name: String,               // enum variant name (stable case)
    pub code: String,               // SCREAMING_SNAKE_CASE of variant (stable)
    pub ty: Option<TypeRef>,        // payload for tuple/struct variants, None for unit
}
```

### Result<T,E> Mapping

- `Result<T, E>` → `returns: T`, `errors: derive_from_enum(E)`
- `Result<(), E>` → `returns: null`, `errors: derive_from_enum(E)`
- Plain `T` → `returns: T`, `errors: []`

## Supported Types (v0.1.1)

### Primitive Types
- `String` - UTF-8 string
- `u8`, `u16`, `u32`, `u64`, `u128` - Unsigned integers
- `i8`, `i16`, `i32`, `i64`, `i128` - Signed integers
- `bool` - Boolean values

### Container Types
- `Vec<T>` - Vector of supported types
- `Option<T>` - Optional types

### Error Types
- Enum variants with `#[derive(AbiType)]`
- Unit variants: `{ name, code, ty: null }`
- Tuple/struct variants: `{ name, code, ty: <payload_type> }`

### Unsupported Types (v0.1.1)
- `f32`, `f64` - Floating point numbers
- `HashMap<K, V>`, `BTreeMap<K, V>` - Maps
- Custom structs (except when used as events)
- Non-enum error types in `Result<T,E>`

## Usage

```rust
use abi_core::{Abi, AbiFunction, AbiTypeRef, FunctionKind, ParameterDirection, AbiParameter, ErrorAbi};

let mut abi = Abi::new(
    "my_module".to_string(),
    "1.0.0".to_string(),
    "1.85.0".to_string(),
    "abc123".to_string(),
);

let function = AbiFunction {
    name: "greet".to_string(),
    kind: FunctionKind::Query,
    parameters: vec![
        AbiParameter {
            name: "name".to_string(),
            ty: AbiTypeRef::String,
            direction: ParameterDirection::Input,
        },
    ],
    returns: Some(AbiTypeRef::String),
    errors: vec![
        ErrorAbi {
            name: "InvalidName".to_string(),
            code: "INVALID_NAME".to_string(),
            ty: Some(AbiTypeRef::String),
        },
    ],
};

abi.add_function(function);

// Serialize to canonical JSON
let mut output = Vec::new();
abi_core::write_canonical(&abi, &mut output)?;
```

## ABI Extraction

Use the `xtask` command to extract ABI files from compiled packages:

```bash
cargo xtask abi extract --package abi_demo --out target/abi/abi.json
```

## Build Integration

The `abi-export` feature enables automatic ABI generation during build:

```bash
# Build with ABI export enabled
cargo build -p abi_demo --target wasm32-unknown-unknown --features abi-export

# ABI will be automatically written to target/abi/abi.json
```

No macro changes required; `abi-export` only affects build-time metadata generation.

## Testing

Run the test suite:

```bash
cargo test -p abi-core
```

The test suite includes:
- Deterministic serialization tests
- Parameter order preservation tests
- Path-free JSON validation
- Type coverage tests
- Error handling tests
- Result<T,E> mapping tests 