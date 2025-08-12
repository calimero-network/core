# ABI Macros

Procedural macros for automatic ABI (Application Binary Interface) generation from Rust code.

## Overview

This crate provides procedural macros that automatically generate ABI definitions from annotated Rust modules. It's designed to work with the `abi-core` crate to produce canonical JSON ABI files.

## Features

- **Module-level ABI Generation**: Generate complete ABI from annotated modules
- **Function Annotations**: Mark functions as queries or commands
- **Event Support**: Define events with structured payloads
- **Advanced Type Support**: Structs, enums, maps, tuples, and arrays
- **Type Derivation**: `#[derive(AbiType)]` for custom types
- **Type Validation**: Compile-time validation of supported types
- **Helpful Error Messages**: Clear diagnostics for common mistakes

## Usage

### Basic Module Definition

```rust
use abi_macros as abi;

#[abi::module(name = "demo", version = "0.1.0")]
pub mod demo {
    use super::*;
    
    /// Query function to get a greeting
    #[abi::query]
    pub fn get_greeting(name: String) -> String {
        format!("Hello, {}!", name)
    }
    
    /// Command function to set a greeting
    #[abi::command]
    pub fn set_greeting(new_value: String) -> Result<(), String> {
        if new_value.is_empty() {
            return Err("Greeting cannot be empty".to_string());
        }
        Ok(())
    }
    
    /// Event emitted when greeting changes
    #[abi::event]
    pub struct GreetingChanged {
        pub old: String,
        pub new: String,
    }
}
```

### Advanced Types with Derive

```rust
use abi_macros as abi;
use std::collections::BTreeMap;

#[abi::module(name = "advanced", version = "0.1.0")]
pub mod advanced {
    use super::*;
    
    /// Custom struct with derive
    #[derive(AbiType)]
    pub struct User {
        pub id: u64,
        pub name: String,
        pub metadata: Option<Vec<u8>>,
    }
    
    /// Custom enum with derive
    #[derive(AbiType)]
    pub enum Status {
        Pending,
        Active(u32),
        Completed { timestamp: u64, result: String },
    }
    
    /// Query using custom types
    #[abi::query]
    pub fn get_user(id: u64) -> Result<User, String> {
        Ok(User {
            id,
            name: "Test User".to_string(),
            metadata: Some(vec![1, 2, 3]),
        })
    }
    
    /// Command using maps and tuples
    #[abi::command]
    pub fn update_status(
        user_id: u64,
        status: Status,
        coords: (u8, String),
    ) -> Result<BTreeMap<String, u64>, String> {
        let mut result = BTreeMap::new();
        result.insert("updated".to_string(), 1234567890);
        Ok(result)
    }
}
```

### Required Attributes

- `#[abi::module(name = "module_name", version = "0.1.0")]` - Required on the module
- `#[abi::query]` - Marks a function as a query (read-only)
- `#[abi::command]` - Marks a function as a command (state-changing)
- `#[abi::event]` - Marks a struct as an event
- `#[derive(AbiType)]` - Enables ABI generation for custom types

### Function Requirements

- Functions must be `pub` (public)
- Parameters must use supported types (see Supported Types)
- Return types must be supported types or `Result<T, E>` for commands

## Supported Types

### Primitive Types
- `String` - UTF-8 string
- `u8`, `u16`, `u32`, `u64`, `u128` - Unsigned integers
- `i8`, `i16`, `i32`, `i64`, `i128` - Signed integers
- `bool` - Boolean values

### Container Types
- `Vec<T>` - Vector of supported types
- `Option<T>` - Optional types

### Advanced Types
- **Structs**: `#[derive(AbiType)]` on structs with named fields
- **Newtype Structs**: `#[derive(AbiType)]` on single-field structs
- **Enums**: `#[derive(AbiType)]` on enums with unit, tuple, or struct variants
- **Maps**: `BTreeMap<K,V>` and `HashMap<K,V>` with dual mode:
  - `Map<String, V>` → `"mode": "object"`
  - `Map<K, V>` (K ≠ String) → `"mode": "entries"`
- **Tuples**: `(T1, T2, ..., Tn)` up to 4 elements
- **Arrays**: `[T; N]` fixed-size arrays

### Unsupported Types
- `f32`, `f64` - Floating point numbers (compile-time error)
- Unions - Not supported in ABI

## Type Registry

When using `#[derive(AbiType)]`, types are automatically added to the ABI's type registry:

```json
{
  "types": {
    "module::User": {
      "kind": "struct",
      "fields": [
        {
          "name": "id",
          "ty": { "type": "u64" }
        },
        {
          "name": "name", 
          "ty": { "type": "string" }
        }
      ],
      "newtype": false
    }
  }
}
```

Functions can then reference these types using `$ref`:

```json
{
  "returns": {
    "$ref": "module::User"
  }
}
```

## Error Diagnostics

The macros provide helpful error messages for common issues:

### Missing Required Attributes
```rust
// ❌ Missing name attribute
#[abi::module(version = "0.1.0")]
pub mod demo { }

// Error: missing required 'name' attribute. Use #[abi::module(name = "module_name", version = "0.1.0")]
```

### Private Functions
```rust
#[abi::module(name = "demo", version = "0.1.0")]
pub mod demo {
    #[abi::query]
    fn get_greeting(name: String) -> String { // ❌ Not public
        format!("Hello, {}!", name)
    }
}

// Error: query functions must be public
```

### Unsupported Types
```rust
#[abi::module(name = "demo", version = "0.1.0")]
pub mod demo {
    #[abi::query]
    pub fn get_greeting(name: f64) -> String { // ❌ f64 not supported
        format!("Hello, {}!", name)
    }
}

// Error: floating point types are not supported in ABI
```

### Missing AbiType Derive
```rust
#[abi::module(name = "demo", version = "0.1.0")]
pub mod demo {
    pub struct User { // ❌ Missing derive
        pub id: u64,
    }
    
    #[abi::query]
    pub fn get_user() -> User { ... }
}

// Error: type User cannot be used in ABI without #[derive(AbiType)]
```

## Generated Output

The macro generates:
1. An ABI JSON file in `target/abi/{module_name}.json`
2. A constant `ABI_PATH` pointing to the generated file

```rust
// Generated constant
pub const ABI_PATH: &str = concat!(env!("OUT_DIR"), "/calimero/abi/demo.json");
```

## Build Integration

Enable automatic ABI generation during build:

```bash
# Build with ABI export enabled
cargo build -p abi_demo --target wasm32-unknown-unknown --features abi-export

# ABI will be automatically written to target/abi/abi.json
```

## Testing

Run the UI tests to verify error diagnostics:

```bash
cargo test -p abi-macros
```

The test suite includes:
- Compile-fail tests for error cases
- Validation of error message content
- Type checking tests
- Advanced types tests
- Derive macro tests 