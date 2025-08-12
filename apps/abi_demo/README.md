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

## Usage

### Building the Demo

```bash
# Build the demo app
cargo build -p abi_demo

# Run tests
cargo test -p abi_demo
```

### Extracting the ABI

```bash
# Extract ABI using xtask
cargo xtask abi extract --module demo --out target/abi/abi.json

# Or manually copy from build output
cp target/debug/build/abi_demo-*/out/calimero/abi/demo.json target/abi/abi.json
```

### Generated ABI

The demo generates an ABI with:
- **Query**: `get_greeting(name: String) -> String`
- **Command**: `set_greeting(new_value: String) -> Result<(), DemoError>`
- **Event**: `GreetingChanged { old: String, new: String }`

## Code Structure

```rust
use abi_macros as abi;

/// Example error type
#[derive(Debug, thiserror::Error)]
pub enum DemoError {
    #[error("Invalid greeting: {0}")]
    InvalidGreeting(String),
}

/// Example SSApp module with ABI generation
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
    pub fn set_greeting(new_value: String) -> Result<(), DemoError> {
        if new_value.is_empty() {
            return Err(DemoError::InvalidGreeting("Greeting cannot be empty".to_string()));
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
    "schema_version": "0.1.0",
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
      "return_type": "String"
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

## Development

This demo serves as a reference implementation for:
- Proper ABI macro usage
- Error handling patterns
- Testing strategies
- ABI extraction workflows

Use this as a starting point for your own SSApp development. 