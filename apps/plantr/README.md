# Plantr Calendar Application

A calendar application built with Calimero SDK for managing calendar events.

## Building

To build the application for WASM:

```bash
rustup target add wasm32-unknown-unknown
cargo build -p plantr --target wasm32-unknown-unknown
```

## ABI Extraction

To extract the ABI from the compiled WASM:

```bash
calimero-abi extract target/wasm32-unknown-unknown/debug/plantr.wasm -o apps/plantr/res/abi.json
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
- Custom types → `$ref` to expanded type definition

## Complex Types

The application defines several complex types that are referenced in the ABI using `$ref`:

- `UserId`: Custom ID type for users
- `CalendarEvent`: Event structure with metadata
- `CreateCalendarEvent`: Input structure for creating events
- `UpdateCalendarEvent`: Input structure for updating events
- Error and Event enums with proper payload types

## Type Expansion

By default, custom types are referenced using `$ref` in the ABI. To expand a type definition in the ABI, use the `#[app::abi_type]` macro:

```rust
#[app::abi_type]
pub struct CalendarEvent {
    id: String,
    title: String,
    // ... other fields
}
```

This will expand the type definition in the ABI's `types` section, making it available for reference by other types. 