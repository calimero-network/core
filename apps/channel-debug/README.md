# Channel Debug Application

A debugging application built with Calimero SDK to test `UnorderedMap<Channel, ChannelInfo>` types and related functionality.

## Purpose

This app is designed to debug and test the following types that were identified as not working as expected:

```rust
channels: UnorderedMap<Channel, ChannelInfo>,

pub struct Channel {
    pub name: String,
}

pub struct ChannelInfo {
    pub messages: Vector<Message>,
    pub channel_type: ChannelType,
    pub read_only: bool,
    pub meta: ChannelMetadata,
    pub last_read: UnorderedMap<UserId, MessageId>,
}
```

## Features

- **Channel Management**: Add and retrieve channels
- **Message Handling**: Add messages to channels and retrieve them
- **Type Testing**: Comprehensive testing of `UnorderedMap` with complex nested types
- **Debugging**: Methods to inspect state and verify type behavior

## Building

To build the application for WASM:

```bash
rustup target add wasm32-unknown-unknown
cargo build -p channel-debug --target wasm32-unknown-unknown
```

Or use the build script:

```bash
./build.sh
```

## ABI Extraction

To extract the ABI from the compiled WASM:

```bash
calimero-abi extract target/wasm32-unknown-unknown/debug/channel_debug.wasm -o apps/channel-debug/res/abi.json
```

## Available Methods

### Core Methods
- `init()` - Initialize the application
- `add_channel(name, channel_type, description, created_by)` - Add a new channel
- `get_channels()` - Get all channels as a BTreeMap
- `get_channel(name)` - Get a specific channel by name
- `channel_count()` - Get the number of channels

### Message Methods
- `add_message(channel_name, content, sender)` - Add a message to a channel
- `get_messages(channel_name)` - Get all messages from a channel

### Debug Methods
- `clear_channels()` - Clear all channels (for testing)

## Type Definitions

All custom types are marked with `#[app::abi_type]` to ensure proper ABI generation:

- `Channel` - Channel identifier with name
- `Message` - Message structure with id, content, sender, and timestamp
- `ChannelType` - Enum for Public, Private, or Direct channels
- `ChannelMetadata` - Metadata about channel creation
- `ChannelInfo` - Complete channel information including messages and read status

## Testing Complex Types

This app specifically tests:
1. `UnorderedMap<Channel, ChannelInfo>` as the main state structure
2. Nested `Vector<Message>` within ChannelInfo
3. Nested `UnorderedMap<UserId, MessageId>` for read tracking
4. Complex enum and struct combinations
5. Proper serialization/deserialization of all types

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
- Custom types → `$ref` to type name (use `#[app::abi_type]` to expand)
