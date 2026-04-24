# KV Store with SharedStorage

Demonstrates the `SharedStorage<T>` primitive (issue #2197): a single-slot
group-writable storage with a mutable writer set.

The app holds a `SharedStorage<LwwRegister<String>>` — a single-slot
register writable by any signer in the writer set. Authority lives at
the collection level (one writer set governs the whole value), unlike
`AuthoredMap` which authors per entry. Rotations are signed by a
current writer.

## Methods

- `set_shared(value)` — replace the single-slot value (writer-only)
- `get_shared()` — read (anyone)
- `rotate_writers(new_writers)` — replace the writer set (current writer only)

## Building

```bash
./build.sh
```

Produces `res/kv_store_with_shared_storage.wasm`.
