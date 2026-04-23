# KV Store with SharedStorage

Demonstrates the `SharedStorage<T>` primitive (issue #2197): a single-slot
group-writable storage with a mutable writer set.

The app holds a `SharedStorage<NestedMap>` keyed at the collection level
(not per-entry, like `AuthoredMap`). Any signer in the writer set can mutate
the value; rotation is signed by a current writer.

## Methods

- `set_shared(key, value)` — write into the inner map (writer-only)
- `get_shared(key)` — read (anyone)
- `rotate_writers(new_writers)` — replace the writer set (current writer only)

## Building

```bash
./build.sh
```

Produces `res/kv_store_with_shared_storage.wasm`.
