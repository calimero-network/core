# ABI Macros

Procedural macros for automatic ABI (Application Binary Interface) generation from Rust code.

## Overview

This crate provides wrapper macros around the existing Calimero SSApp macros that automatically generate ABI definitions when the `abi-export` feature is enabled. It's designed to work with the `abi-core` crate to produce canonical JSON ABI files.

## Features

- **Reuse Existing Macros**: No new macro names required - uses existing `app::*` macros
- **Feature-Gated ABI Export**: ABI generation only when `abi-export` feature is enabled
- **Advanced Type Support**: Structs, enums, maps, tuples, and arrays
- **Type Derivation**: `#[derive(AbiType)]` for custom types
- **Type Validation**: Compile-time validation of supported types
- **Helpful Error Messages**: Clear diagnostics for common mistakes

## Quick Start

```bash
rustup target add wasm32-unknown-unknown
cargo build -p abi_demo --target wasm32-unknown-unknown --features abi-export
```

## Usage

### Basic SSApp Definition

```rust
use calimero_sdk::app;

#[app::state(emits = DemoEvent)]
#[derive(Debug)]
pub struct DemoApp {
    greeting: String,
}

#[derive(Debug)]
#[app::event]
pub enum DemoEvent {
    GreetingChanged { old: String, new: String },
}

#[app::logic]
impl DemoApp {
    #[app::init]
    pub fn init() -> Self {
        Self {
            greeting: "Hello, World!".to_string(),
        }
    }
    
    /// Query function to get a greeting
    pub fn get_greeting(&self, name: String) -> String {
        format!("Hello, {}!", name)
    }
    
    /// Command function to set a greeting
    pub fn set_greeting(&mut self, new_value: String) -> app::Result<(), String> {
        if new_value.is_empty() {
            return Err("Greeting cannot be empty".to_string());
        }
        
        let old_greeting = self.greeting.clone();
        self.greeting = new_value.clone();
        
        app::emit!(DemoEvent::GreetingChanged {
            old: old_greeting,
            new: new_value,
        });
        
        Ok(())
    }
}
```

### Advanced Types with Derive

```rust
use calimero_sdk::app;
use abi_core::AbiType;
use std::collections::BTreeMap;

#[app::state(emits = ConformanceEvent)]
#[derive(Debug)]
pub struct ConformanceApp {
    users: BTreeMap<UserId, ComplexStruct>,
}

#[derive(Debug)]
#[app::event]
pub enum ConformanceEvent {
    UserStatusChanged { user_id: UserId, old_status: Status, new_status: Status },
}

/// Custom struct with derive
#[derive(AbiType)]
pub struct ComplexStruct {
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

/// Newtype struct
#[derive(AbiType)]
pub struct UserId(u128);

#[app::logic]
impl ConformanceApp {
    #[app::init]
    pub fn init() -> Self {
        Self {
            users: BTreeMap::new(),
        }
    }
    
    /// Query using custom types
    pub fn get_user_info(&self, user_id: UserId) -> app::Result<ComplexStruct, String> {
        Ok(ComplexStruct {
            id: user_id.0,
            name: "Test User".to_string(),
            metadata: Some(vec![1, 2, 3, 4]),
        })
    }
    
    /// Command using maps and tuples
    pub fn update_status(
        &mut self,
        user_id: UserId,
        status: Status,
        coords: (u8, String),
    ) -> app::Result<[u16; 4], String> {
        Ok([1, 2, 3, 4])
    }
}
```

### Required Attributes

- `#[app::state(emits = EventType)]` - Required on the state struct
- `#[app::logic]` - Required on the implementation block
- `#[app::init]` - Marks the initialization function
- `#[app::event]` - Marks an enum as an event
- `#[derive(AbiType)]` - Enables ABI generation for custom types

### Function Requirements

- Functions must be `pub` (public)
- Parameters must use supported types (see Supported Types)
- Return types must be supported types or `app::Result<T, E>` for commands

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
- **Tuples**: Up to 4 items
- **Fixed Arrays**: `[T; N]` where N ≤ 4
- **Maps**: `BTreeMap<K, V>` with dual-mode encoding:
  - String keys → `"mode": "object"`
  - Other keys → `"mode": "entries"`

## ABI Export

To generate ABI files, enable the `abi-export` feature:

```toml
[dependencies]
abi-macros = { path = "../../crates/abi-macros", features = ["abi-export"] }
abi-core = { path = "../../crates/abi-core", features = ["abi-export"] }
```

The ABI will be generated to `target/abi/abi.json` with schema version `0.1.1`.

## ABI Schema

The generated ABI uses the following structure:

```json
{
  "metadata": {
    "schema_version": "0.1.1",
    "toolchain_version": "1.75.0",
    "source_hash": "..."
  },
  "module_name": "demo",
  "module_version": "0.1.0",
  "types": {
    // Optional type registry for advanced types
  },
  "functions": {
    "function_name": {
      "name": "function_name",
      "kind": "query|command",
      "parameters": [...],
      "returns": "type|null",
      "errors": [...]
    }
  },
  "events": {
    "event_name": {
      "name": "event_name",
      "payload_type": "type|null"
    }
  }
}
```

## Error Handling

The ABI macros provide clear error messages for common issues:

- **Float types**: "floats are not supported; use fixed-point or string"
- **Missing AbiType**: "missing `#[derive(AbiType)]` for `TypeName`"
- **Unsupported types**: Clear messages about what types are supported

## Testing

Run the tests to verify ABI generation:

```bash
cargo test -p abi-macros
cargo test -p abi-core
cargo test -p abi_demo --features abi-export
``` 