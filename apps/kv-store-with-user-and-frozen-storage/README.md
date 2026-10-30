# kv-store-with-user-and-frozen-storage

A key-value store on the component SDK (`calimero-app-sdk`) with two more kinds
of state beside the shared items, each a table plus a rule (v1 used
`UserStorage` and `FrozenStorage`):

- **Per-account values**: `setUserSimple` / `setUserNested` write rows keyed by
  `env::caller()`'s account and take no account argument, so an account writes
  only its own. `getUserSimpleFor(userKey)` reads anyone's.
- **Frozen values**: `addFrozen(value)` stores the value under the hex SHA-256
  of its borsh encoding (as v1 did, so the hashes are v1's), computed in the
  guest with `sha2`; nothing ever changes or removes one, and the same content
  again returns the same hash. `getFrozen(hashHex)` reads it.

Shared items: `set`, `get`, `entries`, `len`, `getResult`, `getUnchecked` (traps
on a missing key, on purpose), `remove`, `clear`. Every method needs an org
member (or above).

Events: `Inserted`, `Updated`, `Removed`, `Cleared` for the shared items;
`UserSimpleSet { account, value }`, `UserNestedSet { account, key, value }` and
`FrozenAdded { hash, value }`. The user events name the writer's **account**:
v1 named its device (`executor_id`), and there are no devices here. `remove` emits
`Removed` only when the key held an item, and `addFrozen` emits `FrozenAdded` only
the first time a value is stored (`scenarios/no-op-writes.json`).

```bash
cargo test --manifest-path apps/kv-store-with-user-and-frozen-storage/Cargo.toml
```

`tests/scenarios.rs` runs `scenarios/user-storage.json` and
`scenarios/frozen-storage.json`, the former merobox workflows, in the harness;
the cluster runs the same files.
