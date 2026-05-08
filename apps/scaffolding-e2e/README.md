# E2E KV Store

A consolidated test application that combines all backend E2E coverage into a single app. This replaces the need to run multiple separate apps for comprehensive backend testing.

**Note:** XCall (cross-context communication) is tested separately via `apps/xcall-example`.

## Features Covered

This app consolidates the following backend features previously tested across multiple apps:

| Feature Area                 | Methods                                                                                                                                                  | Source App                            |
| ---------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- |
| **KV Operations**            | `set`, `get`, `get_result`, `entries`, `len`, `remove`, `clear`                                                                                          | kv-store                              |
| **Event Handlers**           | `insert_handler`, `update_handler`, `remove_handler`, `clear_handler`, `get_handler_execution_count`                                                     | kv-store-with-handlers                |
| **User Storage (Simple)**    | `set_user_simple`, `get_user_simple`, `get_user_simple_for`                                                                                              | kv-store-with-user-and-frozen-storage |
| **User Storage (Nested)**    | `set_user_nested`, `get_user_nested`                                                                                                                     | kv-store-with-user-and-frozen-storage |
| **Frozen Storage**           | `add_frozen`, `get_frozen`                                                                                                                               | kv-store-with-user-and-frozen-storage |
| **Private Storage**          | `add_secret`, `add_guess`, `my_secrets`, `games`                                                                                                         | private_data                          |
| **Blob API**                 | `upload_file`, `delete_file`, `list_files`, `get_file`, `get_blob_id_b58`, `search_files`                                                                | blobs                                 |
| **Context Admin**            | `add_member`, `kick_member`, `is_member`, `get_all_members`                                                                                              | access-control                        |
| **Nested CRDTs - Counters**  | `increment_counter`, `get_counter`                                                                                                                       | nested-crdt-test                      |
| **Nested CRDTs - Registers** | `set_register`, `get_register`                                                                                                                           | nested-crdt-test                      |
| **Nested CRDTs - Metadata**  | `set_metadata`, `get_metadata`                                                                                                                           | nested-crdt-test                      |
| **Nested CRDTs - Metrics**   | `push_metric`, `get_metric`, `metrics_len`                                                                                                               | nested-crdt-test                      |
| **Nested CRDTs - Tags**      | `add_tag`, `has_tag`, `get_tag_count`                                                                                                                    | nested-crdt-test                      |
| **RGA Document**             | `rga_insert_text`, `rga_delete_text`, `rga_get_text`, `rga_get_length`, `rga_is_empty`, `rga_set_title`, `rga_get_title`, `rga_append_text`, `rga_clear` | collaborative-editor                  |

## Building

### Build WASM only

```bash
./build.sh
```

Output: `res/e2e_kv_store.wasm`

### Build bundle (.mpk)

```bash
./build-bundle.sh
```

Output: `res/e2e-kv-store-1.0.0.mpk`

## State Structure

The app maintains a comprehensive state that covers all backend CRDT types:

```rust
pub struct E2eKvStore {
    // KV Storage
    kv_items: UnorderedMap<String, LwwRegister<String>>,

    // Handler Tracking
    handler_counter: Counter,

    // User Storage
    user_items_simple: UserStorage<LwwRegister<String>>,
    user_items_nested: UserStorage<NestedMap>,

    // Frozen Storage
    frozen_items: FrozenStorage<String>,

    // Private Game (public hash tracking)
    games: UnorderedMap<String, LwwRegister<String>>,

    // Blob Storage
    files: UnorderedMap<String, FileRecord>,
    file_counter: LwwRegister<u64>,
    file_owner: LwwRegister<String>,

    // Nested CRDTs
    crdt_counters: UnorderedMap<String, Counter>,
    crdt_registers: UnorderedMap<String, LwwRegister<String>>,
    crdt_metadata: UnorderedMap<String, UnorderedMap<String, LwwRegister<String>>>,
    crdt_metrics: Vector<Counter>,
    crdt_tags: UnorderedMap<String, UnorderedSet<String>>,

    // RGA Document
    rga_document: ReplicatedGrowableArray,
    rga_edit_count: Counter,
    rga_metadata: UnorderedMap<String, LwwRegister<String>>,
}
```

## Private State

The app also includes private state that is NOT synchronized across nodes:

```rust
#[app::private]
pub struct PrivateSecrets {
    secrets: UnorderedMap<String, String>,
}
```

## Events

The app emits events for all state changes:

- **KV Events**: `Inserted`, `Updated`, `Removed`, `Cleared`
- **User Storage Events**: `UserSimpleSet`, `UserNestedSet`
- **Frozen Storage Events**: `FrozenAdded`
- **Private Game Events**: `SecretSet`, `Guessed`
- **Blob Events**: `FileUploaded`, `FileDeleted`
- **Nested CRDT Events**: `CounterIncremented`, `RegisterSet`, `MetadataSet`, `MetricPushed`, `TagAdded`
- **RGA Events**: `DocumentCreated`, `TextInserted`, `TextDeleted`, `TitleChanged`

## Event Handlers

KV operations emit events with handler names that trigger corresponding handler methods:

- `set` → emits `Inserted` or `Updated` with handler names
- `remove` → emits `Removed` with `remove_handler`
- `clear` → emits `Cleared` with `clear_handler`

Handler methods increment the `handler_counter` (CRDT G-Counter) which can be queried via `get_handler_execution_count()`.

## Testing

The consolidated workflow in `workflows/e2e.yml` covers all feature areas with separate contexts to avoid state interference. See the workflow file for detailed test scenarios.
