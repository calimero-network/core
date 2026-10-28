# kv-store-with-shared-storage

A shared value and a shared map, each writable only by the accounts in its own writer
set, on the component SDK. v1 expressed this with `SharedStorage<T>`, a collection whose
writer set storage checked on every node; here a writer set is rows of the `writers`
table (`(field, account)`, a count per field) and every write checks the caller against
it first. The app owner, who runs `init`, is the first writer of both fields.

The wasmer `SharedStorage` build, which `calimero-node`'s `shared_storage` test runs, is
[`../wasmer-fixtures/kv-store-with-shared-storage`](../wasmer-fixtures/kv-store-with-shared-storage).

## ABI

| Method | Kind | Params | Returns | Notes |
| --- | --- | --- | --- | --- |
| `setShared` | call | `value` | | writers of the value; `SharedSet` |
| `getShared` | view | | `string` | `""` before the first write |
| `mapSet` | call | `key, value` | | writers of the map |
| `mapGet` | view | `key` | `string \| null` | |
| `myAccount` | view | | `string` | the caller's account, as a writer set names it |
| `rotateWriters` | call | `newWriters: [string]` | | writers of the value; replaces the value's set only (the map keeps its own); `WritersRotated { count }` |
| `tryRotateWriters` | call | `newWriters` | `bool` | `false` instead of `NotWriter` |
| `writerCount` | view | | `u64` | the value's writers |

Errors: `NotWriter { field }` (`"value"` or `"map"`), `NoWriters`, `TooManyWriters { max }`.

## Tests

`cargo test` runs `tests/harness.rs` (v1's tests and the refusals) and
`scenarios/shared-storage.json`, the merobox workflow `test_shared_storage.yml` as a
scenario, in the org of `shared-storage.org.json`.
