# ABI Core

Core ABI schema types and canonical serialization for Calimero SDK.

## Overview

This crate provides the core types and serialization logic for generating canonical JSON ABI files from Rust code. It's designed to work with the `abi-macros` crate to produce deterministic, versioned ABI definitions.

## Features

- **Schema Version 0.1.1**: Stable ABI schema with advanced type support
- **Canonical Serialization**: Deterministic JSON output with sorted collections
- **Advanced Types**: Structs, enums, maps, tuples, arrays with optional type registry
- **Dual-Mode Maps**: String keys → object mode, other keys → entries mode
- **Error Handling**: Structured error information with SCREAMING_SNAKE codes
- **Path Hygiene**: No absolute paths or backslashes in generated JSON

## Schema

### ABI Structure

```json
{
  "metadata": {
    "schema_version": "0.1.1",
    "toolchain_version": "1.75.0",
    "source_hash": "a1b2c3d4e5f6789012345678901234567890abcdef1234567890abcdef123456"
  },
  "module_name": "demo",
  "module_version": "0.1.0",
  "types": {
    "module::TypeName": {
      "kind": "struct|enum|map|tuple|array",
      // ... type-specific fields
    }
  },
  "functions": {
    "function_name": {
      "name": "function_name",
      "kind": "query|command",
      "parameters": [
        {
          "name": "param_name",
          "ty": { "type": "string" },
          "direction": "input"
        }
      ],
      "returns": { "type": "string" } | null,
      "errors": [
        {
          "name": "ErrorVariant",
          "code": "ERROR_VARIANT",
          "ty": { "type": "string" } | null
        }
      ]
    }
  },
  "events": {
    "event_name": {
      "name": "event_name",
      "payload_type": { "type": "string" } | null
    }
  }
}
```

### Type System

#### Primitive Types
- `String` → `{ "type": "string" }`
- `u8`, `u16`, `u32`, `u64`, `u128` → `{ "type": "u8" }`, etc.
- `i8`, `i16`, `i32`, `i64`, `i128` → `{ "type": "i8" }`, etc.
- `bool` → `{ "type": "bool" }`

#### Container Types
- `Vec<T>` → `{ "type": "vec", "items": { "type": "T" } }`
- `Option<T>` → `{ "type": "option", "value": { "type": "T" } }`

#### Advanced Types

**Structs**
```json
{
  "kind": "struct",
  "fields": [
    {
      "name": "field_name",
      "ty": { "type": "string" }
    }
  ],
  "newtype": false
}
```

**Enums**
```json
{
  "kind": "enum",
  "variants": [
    {
      "name": "UnitVariant",
      "kind": "unit"
    },
    {
      "name": "TupleVariant",
      "kind": "tuple",
      "items": [{ "type": "u32" }]
    },
    {
      "name": "StructVariant",
      "kind": "struct",
      "fields": [
        {
          "name": "field_name",
          "ty": { "type": "string" }
        }
      ]
    }
  ]
}
```

**Maps (Dual-Mode)**
```json
// String keys → object mode
{
  "kind": "map",
  "key": { "type": "string" },
  "value": { "type": "u64" },
  "mode": "object"
}

// Other keys → entries mode
{
  "kind": "map",
  "key": { "type": "u64" },
  "value": { "type": "string" },
  "mode": "entries"
}
```

**Tuples & Arrays**
```json
// Tuple
{
  "kind": "tuple",
  "items": [
    { "type": "u8" },
    { "type": "string" }
  ]
}

// Array
{
  "kind": "array",
  "item": { "type": "u16" },
  "len": 4
}
```

### Function ABI

Functions use `{ "returns": <TypeRef|null>, "errors": [...] }` structure:

- **Plain return**: `returns: { "type": "string" }`, `errors: []`
- **Unit return**: `returns: null`, `errors: [...]`
- **Result return**: `returns: { "type": "string" }`, `errors: [...]`

Error variants use SCREAMING_SNAKE_CASE codes:
- `InvalidInput` → `"code": "INVALID_INPUT"`
- `NotFound` → `"code": "NOT_FOUND"`

### Type Registry

Advanced types can be referenced using `$ref`:

```json
{
  "types": {
    "module::User": {
      "kind": "struct",
      "fields": [...]
    }
  },
  "functions": {
    "get_user": {
      "returns": {
        "$ref": "module::User"
      }
    }
  }
}
```

## Usage

### Basic Usage

```rust
use abi_core::{Abi, AbiTypeRef, TypeDef};

// Create a new ABI
let mut abi = Abi::new(
    "demo".to_string(),
    "0.1.0".to_string(),
    "1.75.0".to_string(),
    "source_hash".to_string(),
);

// Add types to registry
abi.add_type("demo::User".to_string(), TypeDef::Struct {
    fields: vec![
        FieldDef {
            name: "id".to_string(),
            ty: AbiTypeRef::inline_primitive("u64".to_string()),
        },
        FieldDef {
            name: "name".to_string(),
            ty: AbiTypeRef::inline_primitive("string".to_string()),
        },
    ],
    newtype: false,
});

// Serialize to canonical JSON
let json = serde_json::to_string_pretty(&abi).expect("Failed to serialize");
```

### Canonical Serialization

The crate provides canonical serialization that ensures deterministic output:

```rust
use abi_core::{write_canonical, sha256};

// Write canonical JSON
write_canonical(&mut std::io::stdout(), &abi).expect("Failed to write");

// Get SHA256 hash
let hash = sha256(&abi).expect("Failed to hash");
println!("ABI SHA256: {:x}", hash);
```

## Testing

Run the test suite to verify functionality:

```bash
cargo test -p abi-core
```

The test suite includes:
- Golden fixture tests with SHA256 verification
- Schema validation tests
- Canonical serialization tests
- Path hygiene tests
- Type system tests

## Build Integration

Enable ABI export during build:

```toml
[dependencies]
abi-core = { path = "../abi-core", features = ["abi-export"] }
```

The `abi-export` feature enables build-time ABI generation utilities. 