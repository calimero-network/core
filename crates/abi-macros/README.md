# ABI Macros

Procedural macros for automatic ABI (Application Binary Interface) generation from Rust code.

## Overview

This crate provides procedural macros that automatically generate ABI definitions from annotated Rust modules. It's designed to work with the `abi-core` crate to produce canonical JSON ABI files.

## Features

- **Module-level ABI Generation**: Generate complete ABI from annotated modules
- **Function Annotations**: Mark functions as queries or commands
- **Event Support**: Define events with structured payloads
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

### Required Attributes

- `#[abi::module(name = "module_name", version = "0.1.0")]` - Required on the module
- `#[abi::query]` - Marks a function as a query (read-only)
- `#[abi::command]` - Marks a function as a command (state-changing)
- `#[abi::event]` - Marks a struct as an event

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

// Error: unsupported parameter type. Supported types: String, u8, u16, u32, u64, u128, i8, i16, i32, i64, i128, bool, Vec<T> where T is supported
```

## Generated Output

The macro generates:
1. An ABI JSON file in `target/abi/{module_name}.json`
2. A constant `ABI_PATH` pointing to the generated file

```rust
// Generated constant
pub const ABI_PATH: &str = concat!(env!("OUT_DIR"), "/calimero/abi/demo.json");
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