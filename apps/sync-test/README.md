# Sync Test Application

Comprehensive test application for validating Calimero's synchronization protocol.

## Purpose

This application exercises ALL storage spaces and CRDT types to ensure proper synchronization:

- **Public Storage**: Shared state across all nodes
- **User Storage**: Per-user isolated state  
- **Frozen Storage**: Content-addressed immutable data

## CRDT Types Tested

| Type | Description | Merge Semantics |
|------|-------------|-----------------|
| `LwwRegister<T>` | Last-Write-Wins register | Latest timestamp wins |
| `Counter` | PN-Counter | Positive/negative increments merge |
| `UnorderedMap<K, V>` | Key-value map | Merge by key, delegate to value CRDT |
| `UserStorage<T>` | Per-user data | Isolated by user public key |
| `FrozenStorage<T>` | Immutable blobs | Content-addressed (no merge needed) |

## Operations

### Public Key-Value
- `set(key, value)` - Set a key-value pair
- `get(key)` - Get a value
- `delete(key)` - Delete a key (creates tombstone)
- `batch_set(pairs)` - Batch set multiple pairs
- `entries()` - Get all entries
- `len()` - Get count of entries

### Public Counters
- `counter_inc(name)` - Increment a named counter
- `counter_dec(name)` - Decrement a named counter
- `counter_get(name)` - Get counter value

### Public Stats (Nested CRDT)
- `stats_inc(entity)` - Record increment
- `stats_dec(entity)` - Record decrement
- `stats_get(entity)` - Get (increments, decrements)

### User Storage
- `user_set_simple(value)` - Set current user's value
- `user_get_simple()` - Get current user's value
- `user_set_kv(key, value)` - Set in user's private store
- `user_get_kv(key)` - Get from user's private store
- `user_delete_kv(key)` - Delete from user's private store
- `user_counter_inc()` - Increment user's counter
- `user_counter_get()` - Get user's counter
- `user_get_simple_for(user_key)` - Read another user's value

### Frozen Storage
- `frozen_add(data)` - Add immutable data, returns hash
- `frozen_get(hash_hex)` - Get by hash

### Verification
- `snapshot()` - Get deterministic state snapshot
- `verify(expected)` - Verify state matches expected
- `get_operation_count()` - Total operations performed
- `get_deleted_count()` - Count of deleted keys
- `was_deleted(key)` - Check if key was deleted

### Bulk Operations (Benchmarking)
- `bulk_write(prefix, count, value_size)` - Write N keys
- `bulk_delete(prefix, count)` - Delete N keys
- `bulk_counter_inc(name, count)` - Increment N times

## Building

```bash
./build.sh
```

Output: `res/sync_test.wasm`

## Testing with merobox

See `workflows/` for example test workflows.

## Deterministic Verification

The `snapshot()` method returns a deterministic representation of state that can be compared across nodes:

```json
{
  "public_kv_count": 10,
  "public_kv_entries": {"key1": "value1", ...},
  "public_counter_values": {"counter1": 5, ...},
  "deleted_keys_count": 2,
  "frozen_count": 1,
  "operation_count": 15
}
```

After sync convergence, all nodes should return identical snapshots.
