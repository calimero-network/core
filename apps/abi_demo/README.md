# ABI Demo

Example SSApp demonstrating ABI generation with the Calimero ABI macros.

## Quickstart

```bash
rustup target add wasm32-unknown-unknown
cargo build -p abi_demo --target wasm32-unknown-unknown --features abi-export
# ABI: apps/abi_demo/target/abi/abi.json
# WASM: target/wasm32-unknown-unknown/debug/abi_demo.wasm
```

## Overview

This is a demonstration application showing how to use the `abi-macros` crate to automatically generate ABI definitions from annotated Rust code. It serves as both a working example and a test case for the ABI generation system.

## Features

- **Query Functions**: Read-only functions that return data
- **Command Functions**: State-changing functions that can return errors
- **Events**: Structured events with typed payloads
- **Error Handling**: Proper error types and handling
- **Advanced Types**: ScoreMap (BTreeMap<String, u64>) showcasing v0.1.1 dual-mode maps
- **Schema v0.1.1**: New function structure with `returns` and `errors` fields

## API

- **Query**: `get_greeting(name: String) -> String`
- **Command**: `set_greeting(new_value: String) -> Result<(), DemoError>`
- **Query**: `get_scores() -> ScoreMap`
- **Command**: `set_score(player: String, score: u64) -> Result<(), ()>`
- **Event**: `GreetingChanged { old: String, new: String }`

## Error Types

```rust
pub enum DemoError {
    Empty,
    TooLong { max: u8, got: u8 },
}
```

## Advanced Types

```rust
pub type ScoreMap = BTreeMap<String, u64>; // String keys → object mode in ABI
```

## Building

```bash
# Build the demo app
cargo build -p abi_demo

# Build with ABI export enabled
cargo build -p abi_demo --target wasm32-unknown-unknown --features abi-export

# Build WASM
rustup target add wasm32-unknown-unknown
cargo build -p abi_demo --target wasm32-unknown-unknown --features abi-export
```

## Generated ABI

The demo generates an ABI with schema version `0.1.1` that includes:

- Function returns as `{ "returns": ..., "errors": [...] }` (no `Result` in JSON)
- Advanced types with `"mode":"object"` for ScoreMap
- Proper error handling with SCREAMING_SNAKE codes
- Event definitions with typed payloads

The ABI is written to `apps/abi_demo/target/abi/abi.json` when building with `--features abi-export`. 