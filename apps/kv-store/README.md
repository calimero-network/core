# kv-store

A string key-value store shared by everyone the app admits: the component SDK's basic
example (`calimero-app-sdk`, built for `wasm32-wasip2` into a `calimero:app` component).
State is one `Table<Entry>` keyed by `key` and a `Counter` of the keys; every change emits
an event to the app's whole audience.

The wasmer build of the same store, which the node's own tests, the size gate and the
release's test bundle use, is [`../wasmer-fixtures/kv-store`](../wasmer-fixtures/kv-store).

## ABI

Names are camelCase; `u64` values travel as decimal strings. Errors are
`422 APP_ERROR {code, fields}`.

| Method | Kind | Params | Returns | Notes |
| --- | --- | --- | --- | --- |
| `init` | init | | | the store starts empty |
| `set` | call | `key, value` | | `Inserted` for a new key, `Updated` for an existing one |
| `updateIfExists` | call | `key, value` | `bool` | `false` writes nothing; `Updated` when it writes |
| `getOrInsert` | call | `key, value` | `string` | the value the key holds after the call; `Inserted` when it writes |
| `get` | view | `key` | `string \| null` | |
| `getResult` | view | `key` | `string` | `NotFound { key }` when absent |
| `getUnchecked` | view | `key` | `string` | traps when absent: kept to show a guest panic |
| `entries` | view | | `{key: value}` | ordered by key, up to 1000 |
| `len` | view | | `u64` | |
| `remove` | call | `key` | `string \| null` | the old value; `Removed` only when there was one |
| `clear` | call | | | any member, no confirmation (as in v1); `Cleared` only when the store was not empty |

Events: `Inserted { key, value }`, `Updated { key, value }`, `Removed { key }`, `Cleared`,
to `Audience::All`.

[`GUIDE.md`](GUIDE.md) tells an agent (or a person) how to use the store.

## Build and test

```bash
cargo test                                                       # here: harness + scenarios
cargo run -p calimero-app-build -- apps/kv-store --out kv_store.wasm   # from the repo root
```

`cargo test` builds the component for `wasm32-wasip2` and runs it on the real engine
(`calimero_app_sdk::testing::TestApp`): `tests/harness.rs` is v1's `TestHost` suite, and
`tests/scenarios.rs` runs every `scenarios/*.json` in the org its `.org.json` names. The
scenarios replace the merobox workflows (`simple-store`, `bundle-invitation`,
`workflow-example`); `kv-store.json` covers the rest of the ABI. The crate is standalone
(its own `[workspace]`).
