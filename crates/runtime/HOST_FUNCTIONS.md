# Host Functions Reference

This document catalogs all host functions available to WASM guest code in the Calimero runtime. Host functions are the bridge between sandboxed WebAssembly modules and the host system.

The authoritative source of truth for the import surface is
[`src/logic/imports.rs`](src/logic/imports.rs); every function below is wired
there and implemented under [`src/logic/host_functions/`](src/logic/host_functions/).

### Status Legend

| Marker | Meaning |
|--------|---------|
| _(none)_ | Implemented and callable from guest code today. |
| **BLOCKED** | Wired into the runtime but disabled — the call returns a failure status without performing the operation. |
| **PLANNED** | Documented design intent only. **Not** present in `imports.rs` and **not** importable; calling it will fail to link. See [Planned / Not Yet Implemented](#planned--not-yet-implemented). |

## Table of Contents

- [Memory Exchange Pattern](#memory-exchange-pattern)
- [Function Categories](#function-categories)
  - [System & Panic Handling](#system--panic-handling)
  - [Register Operations](#register-operations)
  - [Context & Identity](#context--identity)
  - [Input/Output](#inputoutput)
  - [Logging & Events](#logging--events)
  - [Storage (Synchronized)](#storage-synchronized)
  - [Storage (Ordered Index)](#storage-ordered-index)
  - [Storage (Private/Local)](#storage-privatelocal)
  - [State Management](#state-management)
  - [CRDT Collections (JS)](#crdt-collections-js)
  - [User & Frozen Storage (JS)](#user--frozen-storage-js)
  - [Blob Operations](#blob-operations)
  - [Utility](#utility)
- [Planned / Not Yet Implemented](#planned--not-yet-implemented)
  - [Context Mutations (Planned)](#context-mutations-planned)
  - [Governance (Planned)](#governance-planned)
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
| `panic_utf8` | `(msg_ptr: u64, location_ptr: u64) -> !` | Handles panic with UTF-8 message. `msg_ptr` is a `sys::Buffer`; `location_ptr` is a full `sys::Location` struct (file/line/column), **not** just a file pointer. |

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

### Storage (Ordered Index)

A **node-local, ordered secondary index** that backs `SortedMap`/`SortedSet`
range and ordered lookups. Unlike synchronized storage, this index is **NOT
replicated** — each node rebuilds it locally. Keys are unhashed
`collection ‖ order_key` byte strings (so lexicographic byte order is the
iteration order); values are 32-byte entry IDs. Backed by the RocksDB
`SortedIndex` column on a node and an in-memory `BTreeMap` for tests.

Write operations return `1` if the change was persisted and `0` otherwise (so
the guest can fall back to rebuilding its index rather than trusting a partial
write). Key/prefix/bound arguments are bounded by `max_storage_key_size` and
values by `max_storage_value_size`, like `storage_write`.

| Function | Signature | Description |
|----------|-----------|-------------|
| `storage_index_set` | `(key_ptr: u64, value_ptr: u64) -> u32` | Insert/overwrite `key -> value`. Returns `1` if persisted, `0` otherwise. |
| `storage_index_remove` | `(key_ptr: u64) -> u32` | Removes a single index key. Returns `1` if persisted, `0` otherwise. |
| `storage_index_remove_prefix` | `(prefix_ptr: u64) -> u32` | Removes every index key beginning with `prefix`. Returns `1` if persisted, `0` otherwise. |
| `storage_index_scan` | `(lo_ptr: u64, hi_ptr: u64, offset: u64, limit: u64, register_id: u64) -> u32` | Scans `[lo, hi)`, skipping `offset` entries, capped at `limit`. `limit` is `n + 1`-encoded: `0` = unbounded, otherwise `limit - 1` results. Encodes results into the register (see below). Returns `1`. |
| `storage_index_last` | `(lo_ptr: u64, hi_ptr: u64, register_id: u64) -> u32` | Reverse seek: encodes the single largest `(key, value)` in `[lo, hi)` into the register (count `0` or `1`). Backs `SortedMap::last` as an `O(log n)` lookup. Returns `1`. |

**Scan register encoding.** `storage_index_scan` and `storage_index_last` write
their results into `register_id` using a length-prefixed, little-endian format:
a `count: u32`, then for each pair `key_len: u32, key, value_len: u32, value`.

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
| `register_js_sdk_root_merge` | `()` | Opts the JS app root into the WASM `__calimero_merge_root_state` sync path (concurrent-writer convergence). `persist_root_state` then stamps the root with the `JsRoot` marker instead of `None`. |

### CRDT Collections (JS)

These functions support JavaScript SDK CRDT collections. All return `i32` status codes.

#### Map Operations

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_map_new` | `(register_id: u64) -> i32` | Creates new CRDT map, ID written to register. |
| `js_crdt_map_new_with_id` | `(id_ptr: u64, register_id: u64) -> i32` | Creates CRDT map at a caller-supplied deterministic 32-byte ID. |
| `js_crdt_map_get` | `(map_id_ptr: u64, key_ptr: u64, register_id: u64) -> i32` | Gets value for key. |
| `js_crdt_map_insert` | `(map_id_ptr: u64, key_ptr: u64, value_ptr: u64, register_id: u64) -> i32` | Inserts key-value pair. |
| `js_crdt_map_remove` | `(map_id_ptr: u64, key_ptr: u64, register_id: u64) -> i32` | Removes key from map. |
| `js_crdt_map_contains` | `(map_id_ptr: u64, key_ptr: u64) -> i32` | Checks if key exists. |
| `js_crdt_map_iter` | `(map_id_ptr: u64, register_id: u64) -> i32` | Iterates over map entries. |

#### Vector Operations

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_vector_new` | `(register_id: u64) -> i32` | Creates new CRDT vector. |
| `js_crdt_vector_new_with_id` | `(id_ptr: u64, register_id: u64) -> i32` | Creates CRDT vector at a caller-supplied deterministic 32-byte ID. |
| `js_crdt_vector_len` | `(vector_id_ptr: u64, register_id: u64) -> i32` | Gets vector length. |
| `js_crdt_vector_push` | `(vector_id_ptr: u64, value_ptr: u64) -> i32` | Appends value to vector. |
| `js_crdt_vector_get` | `(vector_id_ptr: u64, index: u64, register_id: u64) -> i32` | Gets value at index. |
| `js_crdt_vector_pop` | `(vector_id_ptr: u64, register_id: u64) -> i32` | Removes and returns last element. |

#### Set Operations

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_set_new` | `(register_id: u64) -> i32` | Creates new CRDT set. |
| `js_crdt_set_new_with_id` | `(id_ptr: u64, register_id: u64) -> i32` | Creates CRDT set at a caller-supplied deterministic 32-byte ID. |
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
| `js_crdt_lww_new_with_id` | `(id_ptr: u64, register_id: u64) -> i32` | Creates LWW register at a caller-supplied deterministic 32-byte ID. |
| `js_crdt_lww_set` | `(register_id_ptr: u64, value_ptr: u64, has_value: u32) -> i32` | Sets register value. |
| `js_crdt_lww_get` | `(register_id_ptr: u64, register_id: u64) -> i32` | Gets current value. |
| `js_crdt_lww_timestamp` | `(register_id_ptr: u64, register_id: u64) -> i32` | Gets last update timestamp. |

#### Counter Operations

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_counter_new` | `(register_id: u64) -> i32` | Creates new CRDT counter. |
| `js_crdt_counter_new_with_id` | `(id_ptr: u64, register_id: u64) -> i32` | Creates CRDT counter at a caller-supplied deterministic 32-byte ID. |
| `js_crdt_counter_increment` | `(counter_id_ptr: u64) -> i32` | Increments counter. |
| `js_crdt_counter_value` | `(counter_id_ptr: u64, register_id: u64) -> i32` | Gets current counter value. |
| `js_crdt_counter_get_executor_count` | `(counter_id_ptr: u64, executor_ptr: u64, has_executor: u32, register_id: u64) -> i32` | Gets per-executor count. `has_executor` indicates if executor provided. |

The counter operations above wrap a G-Counter (grow-only, unsigned). The PN-Counter operations below add decrement and report a signed (`i64`) value.

#### PN-Counter Operations

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_pncounter_new` | `(register_id: u64) -> i32` | Creates new PN-counter (increment/decrement). |
| `js_crdt_pncounter_new_with_id` | `(id_ptr: u64, register_id: u64) -> i32` | Creates PN-counter at a caller-supplied deterministic 32-byte ID. |
| `js_crdt_pncounter_increment` | `(counter_id_ptr: u64) -> i32` | Increments counter for the current executor. |
| `js_crdt_pncounter_decrement` | `(counter_id_ptr: u64) -> i32` | Decrements counter for the current executor. |
| `js_crdt_pncounter_value` | `(counter_id_ptr: u64, register_id: u64) -> i32` | Gets current signed value (`i64`, little-endian). |
| `js_crdt_pncounter_get_executor_count` | `(counter_id_ptr: u64, executor_ptr: u64, has_executor: u32, register_id: u64) -> i32` | Gets a single executor's net contribution (`positive - negative`, `i64`). `has_executor` indicates if executor provided. |

#### RGA Operations (Collaborative Text)

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_rga_new` | `(register_id: u64) -> i32` | Creates a new Replicated Growable Array. |
| `js_crdt_rga_new_with_id` | `(id_ptr: u64, register_id: u64) -> i32` | Creates an RGA at a caller-supplied deterministic 32-byte ID. |
| `js_crdt_rga_insert` | `(rga_id_ptr: u64, index: u64, value_ptr: u64) -> i32` | Inserts the UTF-8 `value` (a run of one or more codepoints) at codepoint offset `index`. |
| `js_crdt_rga_delete` | `(rga_id_ptr: u64, index: u64) -> i32` | Deletes the codepoint at offset `index`. |
| `js_crdt_rga_get_text` | `(rga_id_ptr: u64, register_id: u64) -> i32` | Gets the full document as UTF-8 bytes. |
| `js_crdt_rga_len` | `(rga_id_ptr: u64, register_id: u64) -> i32` | Gets the codepoint count (`u64`, little-endian). |

`index`/`len` count **Unicode scalar values (codepoints)** — not bytes, and not UTF-16
code units. RGA elements are `char`s, so a multi-byte UTF-8 sequence (e.g. `é`, or an
emoji) is one position. JS SDK wrappers that receive a JS `string` index (which counts
UTF-16 code units) must convert to a codepoint offset before calling these; for
astral-plane characters the two differ.

#### Sorted Map Operations

Same byte API and CRDT semantics as the Map operations above, but `iter` yields entries in ascending key (byte) order.

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_sortedmap_new` | `(register_id: u64) -> i32` | Creates a new ordered CRDT map. |
| `js_crdt_sortedmap_new_with_id` | `(id_ptr: u64, register_id: u64) -> i32` | Creates an ordered map at a caller-supplied deterministic 32-byte ID. |
| `js_crdt_sortedmap_get` | `(map_id_ptr: u64, key_ptr: u64, register_id: u64) -> i32` | Retrieves a value by key. |
| `js_crdt_sortedmap_insert` | `(map_id_ptr: u64, key_ptr: u64, value_ptr: u64, register_id: u64) -> i32` | Inserts/replaces a value, returning any previous value. |
| `js_crdt_sortedmap_remove` | `(map_id_ptr: u64, key_ptr: u64, register_id: u64) -> i32` | Removes a value, returning the previous value. |
| `js_crdt_sortedmap_contains` | `(map_id_ptr: u64, key_ptr: u64) -> i32` | Checks whether a key exists. |
| `js_crdt_sortedmap_iter` | `(map_id_ptr: u64, register_id: u64) -> i32` | Iterates all entries in ascending key order. |

#### Sorted Set Operations

Same byte API and CRDT semantics as the Set operations above, but `iter` yields values in ascending (byte) order.

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_sortedset_new` | `(register_id: u64) -> i32` | Creates a new ordered CRDT set. |
| `js_crdt_sortedset_new_with_id` | `(id_ptr: u64, register_id: u64) -> i32` | Creates an ordered set at a caller-supplied deterministic 32-byte ID. |
| `js_crdt_sortedset_insert` | `(set_id_ptr: u64, value_ptr: u64) -> i32` | Inserts a value (returns 1 if newly added, 0 if already present). |
| `js_crdt_sortedset_contains` | `(set_id_ptr: u64, value_ptr: u64) -> i32` | Checks membership. |
| `js_crdt_sortedset_remove` | `(set_id_ptr: u64, value_ptr: u64) -> i32` | Removes a value (returns 1 if present, 0 otherwise). |
| `js_crdt_sortedset_len` | `(set_id_ptr: u64, register_id: u64) -> i32` | Gets the element count (`u64`, little-endian). |
| `js_crdt_sortedset_iter` | `(set_id_ptr: u64, register_id: u64) -> i32` | Iterates all values in ascending order. |
| `js_crdt_sortedset_clear` | `(set_id_ptr: u64) -> i32` | Clears all values from the set. |

#### Authored Map Operations

An attributed shared-keyspace map with per-entry ownership. Any context member may `insert` a
new key, which **stamps the caller (executor) as the entry's owner**; only that owner may later
`update` or `remove` the entry. Reads are unrestricted. Ownership is derived from the executor
identity the runtime installs per-execution — no identity argument is passed. A non-owner
`update`/`remove` returns `-1` with an ownership error message written to the register.

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_authored_map_new` | `(register_id: u64) -> i32` | Creates a new attributed map. |
| `js_crdt_authored_map_new_with_id` | `(id_ptr: u64, register_id: u64) -> i32` | Creates an attributed map at a caller-supplied deterministic 32-byte ID. |
| `js_crdt_authored_map_insert` | `(map_id_ptr: u64, key_ptr: u64, value_ptr: u64, register_id: u64) -> i32` | Inserts a new key, stamping the caller as owner. Returns `0` on success; `-1` (error in register) if the key already exists. |
| `js_crdt_authored_map_update` | `(map_id_ptr: u64, key_ptr: u64, value_ptr: u64, register_id: u64) -> i32` | Owner-only. Replaces the value at a key. Returns `1` on success; `-1` (error in register) for a non-owner or missing key. |
| `js_crdt_authored_map_remove` | `(map_id_ptr: u64, key_ptr: u64, register_id: u64) -> i32` | Owner-only. Removes a key, returning the previous value (`1`) or `0` if absent; `-1` (error in register) for a non-owner. |
| `js_crdt_authored_map_get` | `(map_id_ptr: u64, key_ptr: u64, register_id: u64) -> i32` | Retrieves a value by key (`1` found, `0` absent). |
| `js_crdt_authored_map_contains` | `(map_id_ptr: u64, key_ptr: u64) -> i32` | Checks whether a key exists (`1`/`0`). |
| `js_crdt_authored_map_owner_of` | `(map_id_ptr: u64, key_ptr: u64, register_id: u64) -> i32` | Writes the entry owner's 32-byte public key to the register (`1`); `0` with a cleared register if the key is absent. |
| `js_crdt_authored_map_owned_by_me` | `(map_id_ptr: u64, key_ptr: u64) -> i32` | Whether the current executor owns the key (`1`/`0`; `0` for absent keys). |
| `js_crdt_authored_map_iter` | `(map_id_ptr: u64, register_id: u64) -> i32` | Iterates all entries (`[count][klen,key,vlen,value]...`). |
| `js_crdt_authored_map_len` | `(map_id_ptr: u64, register_id: u64) -> i32` | Gets the entry count (`u64`, little-endian). |

#### Authored Vector Operations

An attributed ordered shared-keyspace vector with per-slot ownership. Any context member may
`push` a new entry at the tail, which **stamps the caller (executor) as the slot's owner**; only
that owner may later `update` or `tombstone` the slot. There is intentionally no physical remove —
`tombstone` overwrites the slot with an empty value while preserving its position and owner. A
non-owner `update`/`tombstone` returns `-1` with an ownership error message written to the register.

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_authored_vector_new` | `(register_id: u64) -> i32` | Creates a new attributed vector. |
| `js_crdt_authored_vector_new_with_id` | `(id_ptr: u64, register_id: u64) -> i32` | Creates an attributed vector at a caller-supplied deterministic 32-byte ID. |
| `js_crdt_authored_vector_push` | `(vector_id_ptr: u64, value_ptr: u64, register_id: u64) -> i32` | Pushes a value at the tail, stamping the caller as owner. Writes the new index (`u64`, little-endian) to the register; returns `1`. |
| `js_crdt_authored_vector_update` | `(vector_id_ptr: u64, index: u64, value_ptr: u64, register_id: u64) -> i32` | Owner-only. Replaces the value at a slot. Returns `1` on success; `-1` (error in register) for a non-owner or out-of-bounds index. |
| `js_crdt_authored_vector_tombstone` | `(vector_id_ptr: u64, index: u64, register_id: u64) -> i32` | Owner-only. Retracts a slot (overwrites with an empty value). Returns `1` on success; `-1` (error in register) for a non-owner or out-of-bounds index. |
| `js_crdt_authored_vector_get` | `(vector_id_ptr: u64, index: u64, register_id: u64) -> i32` | Retrieves a value by index (`1` found, `0` absent). |
| `js_crdt_authored_vector_owner_of` | `(vector_id_ptr: u64, index: u64, register_id: u64) -> i32` | Writes the slot owner's 32-byte public key to the register (`1`); `0` with a cleared register if the slot is out of bounds. |
| `js_crdt_authored_vector_owned_by_me` | `(vector_id_ptr: u64, index: u64) -> i32` | Whether the current executor owns the slot (`1`/`0`; `0` for out-of-bounds slots). |
| `js_crdt_authored_vector_iter` | `(vector_id_ptr: u64, register_id: u64) -> i32` | Iterates all values in insertion order (`[count][len,value]...`). |
| `js_crdt_authored_vector_len` | `(vector_id_ptr: u64, register_id: u64) -> i32` | Gets the entry count including tombstoned slots (`u64`, little-endian). |

#### Shared Storage Operations

A group-writable single byte value guarded by a rotatable **writer set** (`SharedStorage`,
i.e. `PermissionedStorage<T, WriterSetAcl>` over a byte value). Any member of the writer set may
read and `set` the value; the set is rotated by a current writer via `rotate_writers`. Both `set`
and `rotate_writers` are **writer-gated**: a caller not in the current writer set is rejected with
`-1` and an `ActionNotAllowed` message written to register `0` (the authoritative check is the
merge-time signature verification against the writer set). The executor identity is taken from the
per-execution env, so no identity argument is threaded through. The byte value rides a
last-write-wins register internally so concurrent writes from different writers converge by HLC
timestamp.

A **writer set** crosses the ABI as a buffer of concatenated 32-byte public keys — the caller passes
`count * 32` bytes; `js_crdt_shared_writers` emits the same encoding (decode `len / 32` keys).

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_crdt_shared_new` | `(writers_ptr: u64, frozen: u32, register_id: u64) -> i32` | Creates a new shared byte cell with the given writer set (concatenated 32-byte keys) and `frozen` flag (`0`/`1`). Writes the 32-byte id to the register; returns `0`. |
| `js_crdt_shared_new_with_id` | `(id_ptr: u64, writers_ptr: u64, frozen: u32, register_id: u64) -> i32` | As above, at a caller-supplied deterministic 32-byte id. |
| `js_crdt_shared_set` | `(cell_id_ptr: u64, value_ptr: u64) -> i32` | Writer-gated. Replaces the value. Returns `1` on success; `-1` with an `ActionNotAllowed` message in register `0` for a non-writer. |
| `js_crdt_shared_get` | `(cell_id_ptr: u64, register_id: u64) -> i32` | Reads the current value into the register (`1` found, `0` never-written with a cleared register). |
| `js_crdt_shared_writers` | `(cell_id_ptr: u64, register_id: u64) -> i32` | Writes the current writer set as concatenated 32-byte keys to the register; returns `1`. |
| `js_crdt_shared_writable_by_me` | `(cell_id_ptr: u64) -> i32` | Whether the current executor is in the writer set (`1`/`0`). |
| `js_crdt_shared_is_frozen` | `(cell_id_ptr: u64) -> i32` | Whether the writer set is frozen (`1`/`0`). |
| `js_crdt_shared_rotate_writers` | `(cell_id_ptr: u64, writers_ptr: u64) -> i32` | Writer-gated. Rotates the writer set to the given keys. Returns `1` on success; `-1` with an `ActionNotAllowed` message in register `0` for a non-writer, a frozen cell, or an empty target set. |
| `js_crdt_delete_collection` | `(id_ptr: u64, register_id: u64) -> i32` | Deletes a root-level collection entity by id and unlinks it from the root (cascades the subtree; rejects Frozen; enforces `Shared` writer authority). Used by the JS SDK to reclaim the random-id collection orphaned by deterministic-id reassignment. Returns `1` if an entity was deleted, `0` if none existed (idempotent), or `-1` with an error message in the register. |

> **Deferred (not in this bridge):** per-writer **OpMask** capabilities (`grant_capability` /
> `revoke_capability` / `rotate_writers_scoped`, exposing `WRITE`/`DELETE`/`ADMIN` granularity) and
> **`SharedStorage<Collection>` nesting** (a group-writable map/set/vector rather than a single byte
> value). Both are planned follow-ups.

### User & Frozen Storage (JS)

#### User Storage

Per-user storage keyed by executor identity.

| Function | Signature | Description |
|----------|-----------|-------------|
| `js_user_storage_new` | `(register_id: u64) -> i32` | Creates user storage instance. |
| `js_user_storage_new_with_id` | `(id_ptr: u64, register_id: u64) -> i32` | Creates user storage instance at a caller-supplied deterministic 32-byte ID. |
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
| `js_frozen_storage_new_with_id` | `(id_ptr: u64, register_id: u64) -> i32` | Creates frozen storage instance at a caller-supplied deterministic 32-byte ID. |
| `js_frozen_storage_add` | `(storage_id_ptr: u64, value_ptr: u64, register_id: u64) -> i32` | Adds blob, returns hash. |
| `js_frozen_storage_get` | `(storage_id_ptr: u64, hash_ptr: u64, register_id: u64) -> i32` | Gets blob by hash. |
| `js_frozen_storage_contains` | `(storage_id_ptr: u64, hash_ptr: u64) -> i32` | Checks if hash exists. |

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

### Utility

| Function | Signature | Description |
|----------|-----------|-------------|
| `fetch` | `(url_ptr: u64, method_ptr: u64, headers_ptr: u64, body_ptr: u64, register_id: u64) -> u32` | HTTP fetch. **BLOCKED** — wired into the runtime but disabled; always returns `1` (failure) without performing a request. |
| `random_bytes` | `(dest_ptr: u64)` | Fills buffer with cryptographically random bytes. |
| `time_now` | `(dest_ptr: u64)` | Writes current Unix timestamp (nanoseconds) as `u64` to 8-byte buffer. |
| `ed25519_verify` | `(sig_ptr: u64, pk_ptr: u64, msg_ptr: u64) -> u32` | Verifies Ed25519 signature. Returns `1` if valid, `0` if invalid. |

---

## Planned / Not Yet Implemented

> ⚠️ **None of the functions in this section currently exist.** They are
> **PLANNED** APIs documented as design intent. They are **not** declared in
> [`src/logic/imports.rs`](src/logic/imports.rs) and have **no** implementation
> under [`src/logic/host_functions/`](src/logic/host_functions/). A guest module
> importing any of them will **fail to instantiate** (missing import). The
> signatures below are provisional and may change before these land.

### Context Mutations (Planned)

Intended to queue context membership and lifecycle mutations to be applied after
execution completes.

| Function | Proposed Signature | Intended Behavior |
|----------|--------------------|-------------------|
| `context_create` | `(protocol_ptr: u64, app_id_ptr: u64, args_ptr: u64, alias_ptr: u64)` | Queue context creation. `alias_ptr` may be `0` for no alias. |
| `context_delete` | `(context_id_ptr: u64)` | Queue context deletion. |
| `context_add_member` | `(public_key_ptr: u64)` | Queue adding a member to the current context. |
| `context_remove_member` | `(public_key_ptr: u64)` | Queue removing a member from the current context. |
| `context_is_member` | `(public_key_ptr: u64) -> u32` | Check membership. Would return `1` if member, `0` if not. |
| `context_members` | `(register_id: u64)` | Write a Borsh-encoded `Vec<[u8;32]>` of members to a register. |
| `context_resolve_alias` | `(alias_ptr: u64, register_id: u64) -> u32` | Resolve an alias to a context ID. Would return `1` if found, `0` if not. |

### Governance (Planned)

Intended to let guest code submit and approve governance proposals.

| Function | Proposed Signature | Intended Behavior |
|----------|--------------------|-------------------|
| `send_proposal` | `(actions_ptr: u64, id_ptr: u64)` | Submit a governance proposal. |
| `approve_proposal` | `(approval_ptr: u64)` | Approve an existing proposal. |

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
