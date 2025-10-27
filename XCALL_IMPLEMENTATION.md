# Cross-Context Call (XCall) Implementation

## Overview

This document describes the implementation of cross-context calls (xcalls) in Calimero, which allows contexts to call functions on other contexts locally after execution completes.

## Architecture

The xcall functionality follows the same pattern as the existing `emit` function for events:

### 1. **System Layer** (`crates/sys`)
- Added `XCall<'a>` type with three fields:
  - `context_id: Buffer<'a>` - The 32-byte context ID to call
  - `function: Buffer<'a>` - The function name to execute
  - `params: Buffer<'a>` - JSON-encoded parameters
- Added `xcall()` system function to the WASM imports

### 2. **Runtime Layer** (`crates/runtime`)
- Added `XCall` struct to store queued cross-context calls
- Extended `VMLimits` with xcall limits:
  - `max_xcalls: u64` (default: 100)
  - `max_xcall_function_size: u64` (default: 100 bytes)
  - `max_xcall_params_size: u64` (default: 16 KiB)
- Added `xcalls: Vec<XCall>` field to `VMLogic` to collect calls during execution
- Added `xcalls: Vec<XCall>` to `Outcome` to return collected calls
- Implemented `xcall()` host function with validation and overflow checks
- Added error types: `XCallsOverflow`, `XCallFunctionSizeOverflow`, `XCallParamsSizeOverflow`

### 3. **SDK Layer** (`crates/sdk`)
- Added `xcall()` function to `env` module:
  ```rust
  pub fn xcall(context_id: &[u8; 32], function: &str, params: &[u8])
  ```

### 4. **Context Handler** (`crates/context`)
- Modified execution handler to process xcalls after execution completes
- Xcalls are executed sequentially using `context_client.mutate()`
- Errors are logged but don't fail the main execution
- Added xcall count to execution logging

## Usage Example

See `apps/xcall-example` for a complete working example.

Basic usage:

```rust
use calimero_sdk::env;

// Prepare parameters (must be JSON-encoded)
#[derive(serde::Serialize)]
struct MyParams {
    message: String,
}

let params = serde_json::to_vec(&MyParams {
    message: "Hello!".to_string(),
})?;

// Call another context
let target_context_id = [0u8; 32]; // The context to call
env::xcall(&target_context_id, "my_function", &params);
```

## Key Differences from Events

| Feature | Events (emit) | Cross-Context Calls (xcall) |
|---------|--------------|----------------------------|
| **Broadcast** | Yes, to all nodes | No, executes locally only |
| **Execution** | Handlers run on receiving nodes | Executes immediately after current execution |
| **Use Case** | Notifications, state sync | Direct context-to-context communication |
| **Network** | Distributed across peers | Local to current node |

## Implementation Details

### Collection Phase
During WASM execution, calls to `env::xcall()` are collected in `VMLogic.xcalls` vector, similar to how events are collected.

### Execution Phase
After the main execution completes successfully:
1. The `Outcome` contains all queued xcalls
2. The context handler iterates through `outcome.xcalls`
3. Each xcall is executed via `context_client.mutate()`
4. Execution is logged but errors don't fail the main operation

### Security & Limits
- Maximum number of xcalls per execution (default: 100)
- Maximum function name size (default: 100 bytes)
- Maximum parameters size (default: 16 KiB)
- All limits are configurable via `VMLimits`

## Testing

To test the xcall functionality:

1. Build the example app:
   ```bash
   cd apps/xcall-example
   ./build.sh
   ```

2. Deploy to two contexts (A and B)

3. Call `send_greeting` from Context A with Context B's ID

4. Query `get_messages` on Context B to see the received greeting

## Future Enhancements

Potential improvements:
- Async xcall responses
- Xcall batching for efficiency
- Configurable execution order
- Cross-node xcalls (with proper security considerations)
- Xcall return value handling

