# ABI Demo

Example SSApp demonstrating ABI generation with the Calimero ABI macros.

## Overview

This is a demonstration application showing how to use the `abi-macros` crate to automatically generate ABI definitions from annotated Rust code. It serves as both a working example and a test case for the ABI generation system.

## Features

- **Query Functions**: Read-only functions that return data
- **Command Functions**: State-changing functions that can return errors
- **Events**: Structured events with typed payloads
- **Error Handling**: Proper error types and handling
- **Deterministic ABI**: Consistent output across builds
- **Schema v0.1.1**: New function structure with `returns` and `errors` fields

## Quickstart

```bash
# Add WASM target
rustup target add wasm32-unknown-unknown

# Build with ABI export
cargo build -p abi_demo --target wasm32-unknown-unknown --features abi-export

# ABI written to apps/abi_demo/target/abi/abi.json
```

## Usage

### Building the Demo

```bash
# Build the demo app
cargo build -p abi_demo

# Build with ABI export enabled
cargo build -p abi_demo --target wasm32-unknown-unknown --features abi-export

# Run tests
cargo test -p abi_demo
```

### Extracting the ABI

```bash
# Extract ABI using xtask
cargo xtask abi extract --package abi_demo --out target/abi/abi.json

# Or build with feature flag (ABI automatically written to target/abi/abi.json)
cargo build -p abi_demo --target wasm32-unknown-unknown --features abi-export
```

### Generated ABI

The demo generates an ABI with:
- **Query**: `get_greeting(name: String) -> String`
- **Command**: `set_greeting(new_value: String) -> Result<(), DemoError>`
- **Query**: `compute(value: u64, divisor: u64) -> Result<u64, ComputeError>`
- **Event**: `GreetingChanged { old: String, new: String }`

## Code Structure

```rust
use abi_macros as abi;

/// Example error type for greeting operations
#[derive(Debug, thiserror::Error)]
pub enum DemoError {
    #[error("Invalid greeting: {0}")]
    InvalidGreeting(String),
    #[error("Greeting too long: {0}")]
    GreetingTooLong(usize),
}

/// Example error type for computation operations
#[derive(Debug, thiserror::Error)]
pub enum ComputeError {
    #[error("Division by zero")]
    DivisionByZero,
    #[error("Overflow occurred")]
    Overflow,
    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

/// Example SSApp module with ABI generation
#[abi::module(name = "demo", version = "0.1.0")]
pub mod demo {
    use super::*;
    
    /// Query function to get a greeting (plain T return)
    #[abi::query]
    pub fn get_greeting(name: String) -> String {
        format!("Hello, {}!", name)
    }
    
    /// Command function to set a greeting (Result<(), E> return)
    #[abi::command]
    pub fn set_greeting(new_value: String) -> std::result::Result<(), DemoError> {
        if new_value.is_empty() {
            return Err(DemoError::InvalidGreeting("Greeting cannot be empty".to_string()));
        }
        
        if new_value.len() > 100 {
            return Err(DemoError::GreetingTooLong(new_value.len()));
        }
        
        Ok(())
    }
    
    /// Query function to compute a value (Result<T, E> return)
    #[abi::query]
    pub fn compute(value: u64, divisor: u64) -> std::result::Result<u64, ComputeError> {
        if divisor == 0 {
            return Err(ComputeError::DivisionByZero);
        }
        
        if value > u64::MAX / 2 {
            return Err(ComputeError::Overflow);
        }
        
        if value == 0 {
            return Err(ComputeError::InvalidInput("Value cannot be zero".to_string()));
        }
        
        Ok(value / divisor)
    }
    
    /// Event emitted when greeting changes
    #[abi::event]
    pub struct GreetingChanged {
        pub old: String,
        pub new: String,
    }
}
```

## Testing

### Unit Tests

```bash
cargo test -p abi_demo
```

Tests include:
- Function behavior validation
- Error handling verification
- ABI determinism checks

### Determinism Test

The determinism test ensures that:
- ABI generation is consistent across builds
- Parameter order is preserved
- JSON output is deterministic
- No absolute paths are included

### ABI Structure Test

Validates that the generated ABI has:
- Correct metadata (schema version, toolchain version, source hash)
- Proper module information (name, version)
- Function definitions with correct types
- Event definitions

## ABI Output

The generated ABI file (`target/abi/abi.json`) contains:

```json
{
  "metadata": {
    "schema_version": "0.1.1",
    "toolchain_version": "1.85.0",
    "source_hash": "..."
  },
  "module_name": "demo",
  "module_version": "0.1.0",
  "functions": {
    "get_greeting": {
      "name": "get_greeting",
      "kind": "query",
      "parameters": [
        {
          "name": "name",
          "ty": "String",
          "direction": "input"
        }
      ],
      "returns": {
        "type": "String"
      },
      "errors": []
    },
    "set_greeting": {
      "name": "set_greeting",
      "kind": "command",
      "parameters": [
        {
          "name": "new_value",
          "ty": "String",
          "direction": "input"
        }
      ],
      "errors": [
        {
          "name": "InvalidGreeting",
          "code": "INVALID_GREETING",
          "ty": {
            "type": "String"
          }
        },
        {
          "name": "GreetingTooLong",
          "code": "GREETING_TOO_LONG",
          "ty": {
            "type": "usize"
          }
        }
      ]
    },
    "compute": {
      "name": "compute",
      "kind": "query",
      "parameters": [
        {
          "name": "value",
          "ty": "u64",
          "direction": "input"
        },
        {
          "name": "divisor",
          "ty": "u64",
          "direction": "input"
        }
      ],
      "returns": {
        "type": "u64"
      },
      "errors": [
        {
          "name": "DivisionByZero",
          "code": "DIVISION_BY_ZERO"
        },
        {
          "name": "InvalidInput",
          "code": "INVALID_INPUT",
          "ty": {
            "type": "String"
          }
        },
        {
          "name": "Overflow",
          "code": "OVERFLOW"
        }
      ]
    }
  },
  "events": {
    "GreetingChanged": {
      "name": "GreetingChanged",
      "payload_type": null
    }
  }
}
```

### Schema v0.1.1 Changes

The new ABI schema (v0.1.1) replaces the old `return_type` field with:

- **`returns`**: Success payload type (missing for unit type `()`)
- **`errors`**: Array of error variants with `name`, `code`, and optional `ty` fields

This provides a more universal contract structure that's not tied to Rust's `Result<T,E>` type.

## Development

This demo serves as a reference implementation for:
- Proper ABI macro usage
- Error handling patterns
- Testing strategies
- ABI extraction workflows
- Schema v0.1.1 compliance

Use this as a starting point for your own SSApp development. 