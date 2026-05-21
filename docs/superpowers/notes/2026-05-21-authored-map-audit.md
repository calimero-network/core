# AuthoredMap audit notes

## Storage layout
- Inner collection type: `UnorderedMap<K, V, S>` (`authored_map.rs:51-59`)

  ```rust
  pub struct AuthoredMap<K, V, S: StorageAdaptor = MainStorage>
  where
      K: BorshSerialize + BorshDeserialize,
      V: BorshSerialize + BorshDeserialize,
  {
      #[borsh(bound(serialize = "", deserialize = ""))]
      inner: UnorderedMap<K, V, S>,
      storage: Element,
  }
  ```

- How owner is stored: stamped on the inner-map entry's `Element` metadata via
  `StorageType::User { owner, signature_data: None }`, written at insert time
  through `UnorderedMap::insert_with_storage_type` (`authored_map.rs:136-145`):

  ```rust
  let owner: PublicKey = env::executor_id().into();
  let storage_type = StorageType::User {
      owner,
      signature_data: None,
  };

  let _previous = self
      .inner
      .insert_with_storage_type(k, v, storage_type, None)?;
  ```

  `signature_data` is `None` at WASM time; the real ed25519 signature is
  stamped later in `Interface::save_raw` (`interface.rs:2279-2291`) when the
  executor matches the stored owner.

- Key/index derivation: `compute_id(self.inner.element().id(), k.as_ref())`
  (`authored_map.rs:247-249` — `entry_id`). The same recipe inside
  `UnorderedMap::insert_with_storage_type` (`unordered_map.rs:223`) seeds the
  storage `Id` for each entry, so `owner_of` and `insert` agree on the
  address.

## Method-by-method behaviour

| Method | Signature | Author-touching logic (line range) | Notes |
|---|---|---|---|
| `new` | `fn new() -> Self` | none (`70-75`) | Random ID; no env touch. |
| `new_with_field_name` | `fn new_with_field_name(field_name: &str) -> Self` | none (`79-86`) | Stamps `CrdtType::UserStorage` on the *container* element; doesn't touch executor. |
| `reassign_deterministic_id` | `fn reassign_deterministic_id(&mut self, field_name: &str)` | none (`92-102`) | Same as above; container-only. |
| `insert` | `fn insert(&mut self, k: K, v: V) -> Result<(), StoreError>` | `129-146` — reads `env::executor_id()`, stamps `StorageType::User { owner, signature_data: None }`; rejects duplicate keys with `ActionNotAllowed` | New entry always stamped to executor. No "claim ownership" path. |
| `update` | `fn update(&mut self, k: &K, v: V) -> Result<(), StoreError>` | `156-179` — reads `owner_of(k)`, compares with `env::executor_id().into()`, rejects mismatch with `ActionNotAllowed`; on success mutates value in place via `inner.get_mut(k)` which **preserves the existing Element metadata** including `StorageType::User { owner }` | Value mutates in place via `EntryMut`; owner is *not* re-derived from current executor. |
| `remove` | `fn remove(&mut self, k: &K) -> Result<Option<V>, StoreError>` | `187-200` — owner gate identical to `update`; missing key short-circuits `Ok(None)` rather than erroring | Owner gate before `inner.remove`. |
| `get` | `fn get(&self, k: &K) -> Result<Option<V>, StoreError>` | none (`206-208`) | Reads are unrestricted. |
| `contains` | `fn contains(&self, k: &K) -> Result<bool, StoreError>` | none (`214-216`) | Reads are unrestricted. |
| `owner_of` | `fn owner_of(&self, k: &K) -> Result<Option<PublicKey>, StoreError>` | `222-229` — reads metadata via `Index::<S>::get_metadata(entry_id(k))`, extracts `StorageType::User { owner, .. }` | Public API; used by callers to decide whether to attempt mutation. |
| `entries` | `fn entries(&self) -> Result<impl Iterator<Item = (K, V)> + '_, StoreError>` | none (`235-237`) | Yields values, not owners. |
| `len` | `fn len(&self) -> Result<usize, StoreError>` | none (`243-245`) | |
| `entry_id` | `pub(crate) fn entry_id(&self, k: &K) -> Id` | none (`247-249`) | Helper. |

## Mergeable impl
- Lines: `271-293`
- Body summary: no-op (`fn merge(&mut self, _other: &Self) -> Result<(), _> { Ok(()) }`)
- Why this body shape: the per-entry merge runs through the byte-level
  `CrdtType::UserStorage => Ok(incoming.to_vec())` arm in
  `merge.rs:260-261`, which the dispatcher reaches because the container's
  `crdt_type` is set to `UserStorage` via `new_with_field_name`
  (`authored_map.rs:81`) and the `CrdtMeta` impl (`authored_map.rs:301-303`).
  Per-entry signature verification happens in `Interface::apply_action`
  (`interface.rs:620-820+`, the `StorageType::User` arms for both upsert
  and delete). Delegating to `UnorderedMap::merge` here would *re-stamp*
  remote-only keys as `StorageType::Public` (silently stripping the owner),
  which the doc-comment at `277-289` calls out.

## Tests covered
(11 tests in `#[cfg(test)] mod tests`, lines `312-516`)

- `insert_stamps_current_executor_as_owner`
- `insert_rejects_existing_key`
- `update_by_owner_succeeds`
- `update_by_non_owner_rejected`
- `update_missing_key_errors`
- `remove_by_owner_succeeds`
- `remove_by_non_owner_rejected`
- `remove_missing_key_is_none`
- `different_users_own_disjoint_keys_in_shared_keyspace`
- `owner_of_missing_key_is_none`
- `entries_contains_all_inserted_pairs`

## Surprises / non-obvious behaviour
- `update` does **not** re-derive owner from the current executor — `inner.get_mut`
  fetches the existing entry's storage record, and the `EntryMut`-drop path
  in `UnorderedMap::insert_with_storage_type` re-uses the previously-stored
  metadata. Combined with the owner gate at lines 162-167, the original
  owner is preserved verbatim across updates (a non-owner can't reach the
  mutation path at all). This is load-bearing for the divergence-vs-vector
  question: AuthoredMap and AuthoredVector share this property.
- `remove` on a missing key returns `Ok(None)` instead of `Err(NotFound)`,
  but `update` on a missing key returns `Err(NotFound)`. Asymmetric by
  design (mirrors stdlib `HashMap::remove` vs explicit mutation).
- `Mergeable::merge` is a no-op rather than `unreachable!()`. The module
  doc-comment explains why (container-level merge dispatches to the byte
  path in `merge.rs`), but a future refactor that bypasses the registered
  byte path could silently produce wrong results. The trade-off is that
  `Mergeable` is required for `#[app::state]` nesting and `unreachable!()`
  would panic if anyone wired the trait directly.
- `insert` rejects existing keys rather than replacing them; ownership is
  non-transferable. Existing owner must `remove` first, then the new owner
  `insert`s.
