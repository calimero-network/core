# KV Store with Handlers Application

A key-value store application built with Calimero SDK that demonstrates the use of event handlers for all emitted events.

## Features

This application is identical to the standard kv-store but with all `app::emit!` calls including handler parameters:

- **Inserted events** use the `"insert_handler"` handler
- **Updated events** use the `"update_handler"` handler  
- **Removed events** use the `"remove_handler"` handler
- **Cleared events** use the `"clear_handler"` handler

## Building

To build the application for WASM:

```bash
rustup target add wasm32-unknown-unknown
cargo build -p kv-store-with-handlers --target wasm32-unknown-unknown
```

## ABI Extraction

To extract the ABI from the compiled WASM:

```bash
calimero-abi extract target/wasm32-unknown-unknown/debug/kv_store_with_handlers.wasm -o apps/kv-store-with-handlers/res/abi.json
```

## Event Handlers

All events in this application are emitted with specific handler names:

```rust
// Insert events
app::emit!((Event::Inserted { key: &key, value: &value }, "insert_handler"));

// Update events  
app::emit!((Event::Updated { key: &key, value: &value }, "update_handler"));

// Remove events
app::emit!((Event::Removed { key }, "remove_handler"));

// Clear events
app::emit!((Event::Cleared, "clear_handler"));
```

This demonstrates how the optional handler parameter can be used to route events to specific handlers for processing or logging.
