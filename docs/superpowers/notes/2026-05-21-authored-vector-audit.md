# AuthoredVector audit notes

## Storage layout
- Inner collection type: `Vector<V, S>` (`authored_vector.rs:48-56`)

  ```rust
  pub struct AuthoredVector<V, S: StorageAdaptor = MainStorage>
  where
      V: BorshSerialize + BorshDeserialize,
  {
      #[borsh(bound(serialize = "", deserialize = ""))]
      inner: Vector<V, S>,
      storage: Element,
  }
  ```

- How owner is stored: stamped on the inner-vector entry's `Element`
  metadata via `StorageType::User { owner, signature_data: None }`, written
  at push time through `Vector::push_with_storage_type`
  (`authored_vector.rs:124-131`):

  ```rust
  let owner: PublicKey = env::executor_id().into();
  let storage_type = StorageType::User {
      owner,
      signature_data: None,
  };
  self.inner.push_with_storage_type(value, storage_type)
  ```

  `signature_data: None` matches AuthoredMap — the real signature is
  stamped in `Interface::save_raw` (`interface.rs:2279-2291`).

- Key/index derivation: `Vector::push_with_storage_type` delegates to
  `self.inner.insert_with_storage_type(None, value, storage_type)` (no
  custom id → the inner `Collection` allocates one) then returns
  `self.inner.len()? - 1` as the slot index. `entry_id_at(index)` is the
  inverse lookup: `validate_index_bounds` then `self.inner.nth(index)`
  yields the storage `Id` for the slot (`vector.rs:157-163`).

## Method-by-method behaviour

| Method | Signature | Author-touching logic (line range) | Notes |
|---|---|---|---|
| `new` | `fn new() -> Self` | none (`75-80`) | Random ID. |
| `new_with_field_name` | `fn new_with_field_name(field_name: &str) -> Self` | none (`84-91`) | Stamps `CrdtType::UserStorage` on container element. |
| `reassign_deterministic_id` | `fn reassign_deterministic_id(&mut self, field_name: &str)` | none (`94-101`) | Container only. |
| `push` | `fn push(&mut self, value: V) -> Result<usize, StoreError>` | `124-131` — reads `env::executor_id()`, stamps `StorageType::User { owner, signature_data: None }`; returns assigned index | Returns the slot index (unlike `Vector::push` which returns `()`). |
| `update` | `fn update(&mut self, index: usize, value: V) -> Result<(), StoreError>` | `142-157` — `require_owner(index)` reads metadata, then compares stored owner with `env::executor_id().into()`; on success calls `inner.update(index, value)` which mutates in place via `get_mut`, preserving the `Element` metadata and `StorageType::User { owner }` stamp | Out-of-bounds surfaced as `InvalidData`, missing entry as `NotFound`, owner mismatch as `ActionNotAllowed`. |
| `tombstone` | `fn tombstone(&mut self, index: usize) -> Result<(), StoreError> where V: Default` | `166-171` — delegates to `update(index, V::default())`; same owner gate | No physical removal — slot is preserved with `V::default()` to keep indices stable across concurrent pushes. |
| `get` | `fn get(&self, index: usize) -> Result<Option<V>, StoreError>` | none (`177-179`) | |
| `owner_of` | `fn owner_of(&self, index: usize) -> Result<Option<PublicKey>, StoreError>` | `185-194` — `inner.entry_id_at(index)`, then `Index::<S>::get_metadata`, extract `StorageType::User { owner, .. }` | Returns `None` for out-of-bounds and for entries that lack a User stamp. |
| `iter` | `fn iter(&self) -> Result<impl Iterator<Item = V> + '_, StoreError>` | none (`200-202`) | |
| `len` | `fn len(&self) -> Result<usize, StoreError>` | none (`208-210`) | Includes tombstoned slots. |
| `entry_id_at` (`#[cfg(test)]`) | `pub(crate) fn entry_id_at(&self, index: usize) -> Result<Option<Id>, StoreError>` | none (`213-218`) | Test-only. |
| `require_owner` (private) | `fn require_owner(&self, index: usize) -> Result<(Id, PublicKey), StoreError>` | `220-242` — out-of-bounds → `InvalidData`, missing entry → `NotFound`, missing User stamp → `InvalidData("AuthoredVector entry missing User stamp")` | Private helper used by `update`. |

## Mergeable impl
- Lines: `268-287`
- Body summary: no-op (`fn merge(&mut self, _other: &Self) -> Result<(), _> { Ok(()) }`)
- Why this body shape: identical reasoning to `AuthoredMap`. The container's
  `crdt_type` is `UserStorage` (set via `new_with_field_name` at line 86 and
  the `CrdtMeta` impl at 294-296), so the dispatcher in
  `merge.rs:260-261` (`CrdtType::UserStorage => Ok(incoming.to_vec())`)
  handles container-level merge byte-wise, while per-entry signature
  verification runs in `Interface::apply_action`. Delegating to
  `Vector::merge` would call `Vector::push` for any slot present only in
  `other`, which stamps `StorageType::Public` and silently strips
  ownership (per the doc comment at lines 273-283).

## Tests covered
(9 tests in `#[cfg(test)] mod tests`, lines `305-451`)

- `push_stamps_current_executor_as_owner`
- `concurrent_pushes_from_two_users_preserve_per_entry_owner`
- `update_by_owner_succeeds`
- `update_by_non_owner_rejected`
- `update_out_of_bounds_errors`
- `tombstone_by_owner_writes_default`
- `tombstone_by_non_owner_rejected`
- `iter_yields_all_values_in_insertion_order`
- `owner_of_out_of_bounds_is_none`

## Surprises / non-obvious behaviour
- No physical `remove(idx)`. The module-level doc-comment explains why:
  shifting indices would complicate concurrent-push merge semantics, so the
  primitive is `tombstone(idx)` which writes `V::default()` in place. The
  slot, the index, and the `StorageType::User { owner }` stamp are all
  preserved.
- `len` counts tombstones (`208-210` plus the comment).
- `Data::collections()` returns an empty `BTreeMap` (`245-257`) — explicit
  override with a comment explaining that `Vector<V>` does not implement
  `Data` (unlike `UnorderedMap`, which AuthoredMap delegates to), so we
  can't delegate.
- `Debug` is hand-rolled (`58-68`) rather than `#[derive(Debug)]`, omitting
  the inner vector to avoid the `S: StorageAdaptor` `Debug` bound that
  `Vector<V, S>` would propagate.
- `update` preserves the original owner verbatim across mutations — same
  in-place `get_mut` path that AuthoredMap uses. Confirmed by tests
  `update_by_owner_succeeds` (`357-366`) and the asymmetric `tombstone`
  test (`397-409`) which re-checks `owner_of(0)` after tombstoning.
- `require_owner` produces `InvalidData("…missing User stamp")` if metadata
  exists but isn't `User` — paranoia check that can only fire if the inner
  vector's stamping has been bypassed.
- `update` returns `Err(NotFound)` if the entry vanishes between
  `require_owner` and `inner.update` (line 155); racy but defensive.
