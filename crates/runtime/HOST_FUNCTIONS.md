# Host Functions Reference

This document catalogs all host functions available to WASM guest code in the Calimero runtime. Host functions are the bridge between sandboxed WebAssembly modules and the host system.

## Table of Contents

- [Memory Exchange Pattern](#memory-exchange-pattern)
- [Function Categories](#function-categories)
  - [System & Panic Handling](#system--panic-handling)
  - [Register Operations](#register-operations)
  - [Context & Identity](#context--identity)
  - [Input/Output](#inputoutput)
  - [Logging & Events](#logging--events)
  - [Storage (Synchronized)](#storage-synchronized)
  - [Storage (Private/Local)](#storage-privatelocal)
  - [State Management](#state-management)
  - [CRDT Collections (JS)](#crdt-collections-js)
  - [User & Frozen Storage (JS)](#user--frozen-storage-js)
  - [Context Mutations](#context-mutations)
  - [Blob Operations](#blob-operations)
  - [Governance](#governance)
  - [Utility](#utility)
- [Return Conventions](#return-conventions)
- [Error Handling](#error-handling)

---

## Memory Exchange Pattern

All data exchange between guest WASM code and host functions uses a **pointer-based buffer descriptor** pattern:

```
┌─────────────────────────────────────────────────────────────────────┐
│                        GUEST WASM MEMORY                            │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│   Buffer Descriptor (16 bytes)          Actual Data                 │
│   ┌──────────────────────────┐         ┌──────────────────┐        │
│   │ ptr: u64  │  len: u64    │ ──────► │ byte data...     │        │
│   └──────────────────────────┘         └──────────────────┘        │
│                                                                     │
│   Host reads descriptor at given pointer, then reads/writes data    │
│   at the location specified by the descriptor.                      │
└─────────────────────────────────────────────────────────────────────┘
```

### Key Types

| Type | Definition | Usage |
|------|------------|-------|
| `sys::Buffer<'a>` | `Slice<'a, u8>` = `{ ptr: Pointer<u8>, len: u64 }` | Read-only data from guest |
| `sys::BufferMut<'a>` | Same as Buffer | Writable data buffer |
| `sys::Location<'a>` | `{ file: Buffer, line: u32, column: u32 }` | Source location for panics |
| `sys::Event<'a>` | `{ kind: Buffer, data: Buffer }` | Structured event data |
| `sys::XCall<'a>` | `{ context_id: Buffer, function: Buffer, params: Buffer }` | Cross-context call |
| `sys::ValueReturn<'a>` | `Ok(Buffer) \| Err(Buffer)` | Function return value |

### Memory Access Flow

1. **Guest** allocates memory and writes data
2. **Guest** creates a buffer descriptor pointing to the data
3. **Guest** calls host function with pointer to descriptor
4. **Host** reads descriptor via `read_guest_memory_typed::<sys::Buffer>`
5. **Host** validates length against limits
6. **Host** reads actual data via `read_guest_memory_slice`

---

## Function Categories

### System & Panic Handling

| Function | Signature | Description |
|----------|-----------|-------------|
| `panic` | `(location_ptr: u64) -> !` | Handles simple panic without message. Captures file/line/column from `sys::Location`. |
| `panic_utf8` | `(msg_ptr: u64, file_ptr: u64) -> !` | Handles panic with UTF-8 message and file location. |

### Register Operations

Registers are host-side temporary storage slots that allow passing data larger than WASM return values.

| Function | Signature | Description |
|----------|-----------|-------------|
| `register_len` | `(register_id: u64) -> u64` | Returns length of data in register. Returns `u64::MAX` if register doesn't exist. |
| `read_register` | `(register_id: u64, dest_ptr: u64) -> u32` | Copies register data to guest buffer. Returns `1` on success, `0` on length mismatch. |

### Context & Identity

| Function | Signature | Description |
|----------|-----------|-------------|
| `context_id` | `(register_id: u64)` | Writes 32-byte context ID to register. |
| `executor_id` | `(register_id: u64)` | Writes 32-byte executor public key to register. |

### Input/Output

| Function | Signature | Description |
|----------|-----------|-------------|
| `input` | `(register_id: u64)` | Copies execution input data to register. |
| `value_return` | `(value_ptr: u64)` | Sets final return value (`Ok` or `Err` variant). |

### Logging & Events

| Function | Signature | Description |
|----------|-----------|-------------|
| `log_utf8` | `(log_ptr: u64)` | Logs a UTF-8 message (buffer descriptor). |
| `js_std_d_print` | `(ctx_ptr: u64, message_ptr: u64, message_len: u64) -> u32` | QuickJS debug print handler. |
| `emit` | `(event_ptr: u64)` | Emits structured event with `kind` and `data`. |
| `emit_with_handler` | `(event_ptr: u64, handler_ptr: u64)` | Emits event with optional callback handler name. |
| `xcall` | `(xcall_ptr: u64)` | Queues cross-context call for post-execution. |

### Storage (Synchronized)

These operations persist to synchronized storage that replicates across nodes.

| Function | Signature | Description |
|----------|-----------|-------------|
| `storage_read` | `(key_ptr: u64, register_id: u64) -> u32` | Reads value into register. Returns `1` if found, `0` if not. |
| `storage_write` | `(key_ptr: u64, value_ptr: u64, register_id: u64) -> u32` | Writes key-value. Returns `1` if key existed (old value in register), `0` if new. |
| `storage_remove` | `(key_ptr: u64, register_id: u64) -> u32` | Removes key. Returns `1` if existed (old value in register), `0` if not. |

### Storage (Private/Local)

Node-local storage that is **NOT synchronized** across the network.

| Function | Signature | Description |
|----------|-----------|-------------|
| `private_storage_read` | `(key_ptr: u64, register_id: u64) -> u32` | Reads from private storage. Returns `1` if found, `0` if not or unavailable. |
| `private_storage_write` | `(key_ptr: u64, value_ptr: u64) -> u32` | Writes to private storage. Returns `1` on success, `0` if unavailable. |
| `private_storage_remove` | `(key_ptr: u64, register_id: u64) -> u32` | Removes from private storage. Returns `1` if found, `0` if not. |

### State Management

| Function | Signature | Description |
|----------|-----------|-------------|
| `commit` | `(root_hash_ptr: u64, artifact_ptr: u64)` | Commits execution state with 32-byte root hash and artifact. **Must be called exactly once.** |
| `persist_root_state` | `(doc_ptr: u64, created_at: u64, updated_at: u64)` | Persists root state document through Merkle tree. |
| `read_root_state` | `(register_id: u64) -> i32` | Reads persisted root state. Returns `1` if exists, `0` if not. |
| `apply_storage_delta` | `(delta_ptr: u64)` | Applies Borsh-encoded `StorageDelta::Actions` from another executor. |
| `flush_delta` | `() -> i32` | Flushes pending CRDT actions as causal delta. Returns `1` if delta emitted, `0` if nothing to commit. |

### CRDT Collections (JS)

These functions support JavaScript SDK CRDT collections. All return `i32` status codes.

#### Map Operations

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_map_new` | `(register_id: u64) -> i32` | Creates new CRDT map, ID written to register. |
| `js_crdt_map_get` | `(map_id_ptr: u64, key_ptr: u64, register_id: u64) -> i32` | Gets value for key. |
| `js_crdt_map_insert` | `(map_id_ptr: u64, key_ptr: u64, value_ptr: u64, register_id: u64) -> i32` | Inserts key-value pair. |
| `js_crdt_map_remove` | `(map_id_ptr: u64, key_ptr: u64, register_id: u64) -> i32` | Removes key from map. |
| `js_crdt_map_contains` | `(map_id_ptr: u64, key_ptr: u64) -> i32` | Checks if key exists. |
| `js_crdt_map_iter` | `(map_id_ptr: u64, register_id: u64) -> i32` | Iterates over map entries. |

#### Vector Operations

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_vector_new` | `(register_id: u64) -> i32` | Creates new CRDT vector. |
| `js_crdt_vector_len` | `(vector_id_ptr: u64, register_id: u64) -> i32` | Gets vector length. |
| `js_crdt_vector_push` | `(vector_id_ptr: u64, value_ptr: u64) -> i32` | Appends value to vector. |
| `js_crdt_vector_get` | `(vector_id_ptr: u64, index: u64, register_id: u64) -> i32` | Gets value at index. |
| `js_crdt_vector_pop` | `(vector_id_ptr: u64, register_id: u64) -> i32` | Removes and returns last element. |

#### Set Operations

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_set_new` | `(register_id: u64) -> i32` | Creates new CRDT set. |
| `js_crdt_set_insert` | `(set_id_ptr: u64, value_ptr: u64) -> i32` | Inserts value into set. |
| `js_crdt_set_contains` | `(set_id_ptr: u64, value_ptr: u64) -> i32` | Checks if value exists. |
| `js_crdt_set_remove` | `(set_id_ptr: u64, value_ptr: u64) -> i32` | Removes value from set. |
| `js_crdt_set_len` | `(set_id_ptr: u64, register_id: u64) -> i32` | Gets set size. |
| `js_crdt_set_iter` | `(set_id_ptr: u64, register_id: u64) -> i32` | Iterates over set values. |
| `js_crdt_set_clear` | `(set_id_ptr: u64) -> i32` | Clears all values from set. |

#### LWW Register Operations

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_lww_new` | `(register_id: u64) -> i32` | Creates new Last-Writer-Wins register. |
| `js_crdt_lww_set` | `(register_id_ptr: u64, value_ptr: u64, has_value: u32) -> i32` | Sets register value. |
| `js_crdt_lww_get` | `(register_id_ptr: u64, register_id: u64) -> i32` | Gets current value. |
| `js_crdt_lww_timestamp` | `(register_id_ptr: u64, register_id: u64) -> i32` | Gets last update timestamp. |

#### Counter Operations

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_counter_new` | `(register_id: u64) -> i32` | Creates new CRDT counter. |
| `js_crdt_counter_increment` | `(counter_id_ptr: u64) -> i32` | Increments counter. |
| `js_crdt_counter_value` | `(counter_id_ptr: u64, register_id: u64) -> i32` | Gets current counter value. |
| `js_crdt_counter_get_executor_count` | `(counter_id_ptr: u64, executor_ptr: u64, has_executor: u32, register_id: u64) -> i32` | Gets per-executor count. `has_executor` indicates if executor provided. |

### User & Frozen Storage (JS)

#### User Storage

Per-user storage keyed by executor identity.

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_user_storage_new` | `(register_id: u64) -> i32` | Creates user storage instance. |
| `js_user_storage_insert` | `(storage_id_ptr: u64, value_ptr: u64, register_id: u64) -> i32` | Inserts value for current user. |
| `js_user_storage_get` | `(storage_id_ptr: u64, register_id: u64) -> i32` | Gets current user's value. |
| `js_user_storage_get_for_user` | `(storage_id_ptr: u64, user_key_ptr: u64, register_id: u64) -> i32` | Gets specific user's value. |
| `js_user_storage_remove` | `(storage_id_ptr: u64, register_id: u64) -> i32` | Removes current user's value. |
| `js_user_storage_contains` | `(storage_id_ptr: u64) -> i32` | Checks if current user has value. |
| `js_user_storage_contains_user` | `(storage_id_ptr: u64, user_key_ptr: u64) -> i32` | Checks if specific user has value. |

#### Frozen Storage

Content-addressable storage for immutable blobs.

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_frozen_storage_new` | `(register_id: u64) -> i32` | Creates frozen storage instance. |
| `js_frozen_storage_add` | `(storage_id_ptr: u64, value_ptr: u64, register_id: u64) -> i32` | Adds blob, returns hash. |
| `js_frozen_storage_get` | `(storage_id_ptr: u64, hash_ptr: u64, register_id: u64) -> i32` | Gets blob by hash. |
| `js_frozen_storage_contains` | `(storage_id_ptr: u64, hash_ptr: u64) -> i32` | Checks if hash exists. |

### Context Mutations

These queue mutations to be applied after execution completes.

| Function | Signature | Description |
|----------|-----------|-------------|
| `context_create` | `(protocol_ptr: u64, app_id_ptr: u64, args_ptr: u64, alias_ptr: u64)` | Queues context creation. `alias_ptr` can be `0` for no alias. |
| `context_delete` | `(context_id_ptr: u64)` | Queues context deletion. |
| `context_add_member` | `(public_key_ptr: u64)` | Queues adding member to current context. |
| `context_remove_member` | `(public_key_ptr: u64)` | Queues removing member from current context. |
| `context_is_member` | `(public_key_ptr: u64) -> u32` | Checks membership. Returns `1` if member, `0` if not. |
| `context_members` | `(register_id: u64)` | Writes Borsh-encoded `Vec<[u8;32]>` of members to register. |
| `context_resolve_alias` | `(alias_ptr: u64, register_id: u64) -> u32` | Resolves alias to context ID. Returns `1` if found, `0` if not. |

### Blob Operations

Large binary object streaming.

| Function | Signature | Description |
|----------|-----------|-------------|
| `blob_create` | `() -> u64` | Opens new blob for writing, returns file descriptor. |
| `blob_write` | `(fd: u64, data_ptr: u64) -> u64` | Writes data to blob, returns bytes written. |
| `blob_close` | `(fd: u64, blob_id_ptr: u64) -> u32` | Closes blob, writes blob ID to buffer. |
| `blob_open` | `(blob_id_ptr: u64) -> u64` | Opens existing blob for reading, returns file descriptor. |
| `blob_read` | `(fd: u64, data_ptr: u64) -> u64` | Reads data from blob into buffer. |
| `blob_announce_to_context` | `(blob_id_ptr: u64, context_id_ptr: u64) -> u32` | Announces blob availability to context. |

### Governance

| Function | Signature | Description |
|----------|-----------|-------------|
| `send_proposal` | `(actions_ptr: u64, id_ptr: u64)` | Submits governance proposal. |
| `approve_proposal` | `(approval_ptr: u64)` | Approves existing proposal. |

### Utility

| Function | Signature | Description |
|----------|-----------|-------------|
| `fetch` | `(url_ptr, method_ptr, headers_ptr, body_ptr, register_id) -> u32` | HTTP fetch. **Currently BLOCKED** - always returns `1` (failure). |
| `random_bytes` | `(dest_ptr: u64)` | Fills buffer with cryptographically random bytes. |
| `time_now` | `(dest_ptr: u64)` | Writes current Unix timestamp (nanoseconds) as `u64` to 8-byte buffer. |
| `ed25519_verify` | `(sig_ptr: u64, pk_ptr: u64, msg_ptr: u64) -> u32` | Verifies Ed25519 signature. Returns `1` if valid, `0` if invalid. |

---

## Return Conventions

| Return Value | Meaning |
|--------------|---------|
| `0` | Operation completed but item not found / no change |
| `1` | Success / item found / change occurred |
| `u64::MAX` | Register not found (for `register_len`) |
| `-1` (i32) | Error occurred (for some JS CRDT functions) |

## Error Handling

Host functions can fail with these common errors:

| Error | Cause |
|-------|-------|
| `InvalidMemoryAccess` | Buffer pointer out of bounds or invalid descriptor |
| `KeyLengthOverflow` | Key exceeds `max_storage_key_size` |
| `ValueLengthOverflow` | Value exceeds `max_storage_value_size` |
| `LogsOverflow` | Too many log messages |
| `LogLengthOverflow` | Log message too long |
| `EventsOverflow` | Too many events emitted |
| `EventKindSizeOverflow` | Event kind string too long |
| `EventDataSizeOverflow` | Event data too large |
| `BadUTF8` | String buffer contains invalid UTF-8 |
| `InvalidRegisterId` | Requested register doesn't exist |
| `DeserializationError` | Borsh deserialization failed |
| `Panic` | Guest triggered panic |

---

## Resource Limits

All operations are bounded by `VMLimits`:

| Limit | Default | Description |
|-------|---------|-------------|
| `max_memory_pages` | 1024 | Maximum WASM memory pages (64KB each = 64MB total) |
| `max_stack_size` | 200KB | Maximum stack size |
| `max_registers` | 100 | Maximum number of registers |
| `max_register_size` | 100MB | Maximum size per register |
| `max_storage_key_size` | 1MB | Maximum storage key length |
| `max_storage_value_size` | 10MB | Maximum storage value length |
| `max_logs` | 100 | Maximum log messages |
| `max_log_size` | 16KB | Maximum log message length |
| `max_events` | 100 | Maximum events |
| `max_event_kind_size` | 100 | Maximum event kind length (bytes) |
| `max_event_data_size` | 16KB | Maximum event data size |
