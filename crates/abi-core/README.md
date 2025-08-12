# ABI Core

Core types and serialization for Calimero ABI (Application Binary Interface) generation.

## Overview

This crate provides the foundational types and serialization logic for generating ABIs from Rust code. It's designed to work with the `abi-macros` crate to automatically generate ABI definitions from annotated Rust modules.

## Features

- **Canonical JSON Serialization**: Deterministic JSON output for ABI files
- **SHA256 Hashing**: Cryptographic hashing of ABI definitions
- **Type System**: Comprehensive type definitions for ABI parameters and return values
- **Cross-Platform**: No absolute paths or platform-specific separators in output

## Supported Types (v0.1)

### Primitive Types
- `String` - UTF-8 string
- `u8`, `u16`, `u32`, `u64`, `u128` - Unsigned integers
- `i8`, `i16`, `i32`, `i64`, `i128` - Signed integers
- `bool` - Boolean values

### Container Types
- `Vec<T>` - Vector of supported types

### Unsupported Types (v0.1)
- `f32`, `f64` - Floating point numbers
- `HashMap<K, V>`, `BTreeMap<K, V>` - Maps
- `Option<T>` - Optional types
- Custom structs (except when used as events)
- Enums (except when used as events)

## Usage

```rust
use abi_core::{Abi, AbiFunction, AbiTypeRef, FunctionKind, ParameterDirection, AbiParameter};

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
    return_type: Some(AbiTypeRef::String),
};

abi.add_function(function);

// Serialize to canonical JSON
let mut output = Vec::new();
abi_core::write_canonical(&abi, &mut output)?;
```

## ABI Extraction

Use the `xtask` command to extract ABI files from compiled modules:

```bash
cargo xtask abi extract --module demo --out target/abi/abi.json
```

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