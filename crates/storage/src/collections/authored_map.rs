//! Shared-keyspace map with per-entry ownership.
//!
//! `AuthoredMap<K, V>` exposes an `UnorderedMap<K, V>` whose entries each carry
//! a `StorageType::User { owner }` stamp set to the inserter's public key.
//! Any context member can insert a new key; only the inserter can update or
//! remove their own entries. Reads are unrestricted.
//!
//! The per-entry authorization is enforced at merge time in
//! `Interface::apply_action` (see `interface.rs`). Local `update`/`remove`
//! additionally short-circuit non-owner calls so bugs surface in-process.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

use super::crdt_meta::{CrdtMeta, CrdtType, Mergeable, StorageStrategy};
use super::{compute_id, StoreError, UnorderedMap};
use crate::entities::{ChildInfo, Data, Element, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::StorageError;
use crate::store::{MainStorage, StorageAdaptor};

/// A map keyed by `K` where each entry is owned by the public key that
/// inserted it.
///
/// Internally an `UnorderedMap<K, V>`. Each entry's `StorageType` is
/// `User { owner }`, set at insert time from `env::executor_id()`. Only the
/// owner can `update` or `remove` their entry.
#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct AuthoredMap<K, V, S: StorageAdaptor = MainStorage>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
{
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: UnorderedMap<K, V, S>,
    storage: Element,
}

impl<K, V> AuthoredMap<K, V, MainStorage>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
{
    /// Creates a new, empty `AuthoredMap` with a random ID.
    ///
    /// Use this for nested collections. For top-level state fields, use
    /// `new_with_field_name`.
    pub fn new() -> Self {
        Self {
            inner: UnorderedMap::new(),
            storage: Element::new(None),
        }
    }

    /// Creates a new, empty `AuthoredMap` with a deterministic ID derived from
    /// `field_name`.
    pub fn new_with_field_name(field_name: &str) -> Self {
        let mut storage = Element::new_with_field_name(None, Some(field_name.to_string()));
        storage.metadata.crdt_type = Some(CrdtType::UserStorage);
        Self {
            inner: UnorderedMap::new_with_field_name(&format!("__authored_map_{field_name}")),
            storage,
        }
    }

    /// Reassigns the collection's ID deterministically based on `field_name`.
    ///
    /// Used by the `#[app::state]` macro after `init()` so top-level collections
    /// have stable IDs across nodes.
    pub fn reassign_deterministic_id(&mut self, field_name: &str)
    where
        K: AsRef<[u8]> + PartialEq,
    {
        use super::compute_collection_id;
        let new_id = compute_collection_id(None, field_name);
        self.storage.reassign_id_and_field_name(new_id, field_name);
        self.storage.metadata.crdt_type = Some(CrdtType::UserStorage);
        self.inner
            .reassign_deterministic_id(&format!("__authored_map_{field_name}"));
    }
}

impl<K, V> Default for AuthoredMap<K, V, MainStorage>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, S> AuthoredMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Inserts a new entry, stamping the current executor as its owner.
    ///
    /// Fails with `ActionNotAllowed` if `k` is already present. Ownership
    /// transfer is not supported — use `remove` + `insert` from the new owner.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if `k` already exists, or any underlying
    /// storage error.
    pub fn insert(&mut self, k: K, v: V) -> Result<(), StoreError> {
        if self.inner.contains(&k)? {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "AuthoredMap::insert: key already exists".to_owned(),
            )));
        }

        let owner: PublicKey = env::executor_id().into();
        let storage_type = StorageType::User {
            owner,
            signature_data: None,
        };

        let _previous = self
            .inner
            .insert_with_storage_type(k, v, storage_type, None)?;
        Ok(())
    }

    /// Replaces the value at `k`. Only the entry's owner may call this.
    ///
    /// Fails with `NotFound` if `k` is absent, or `ActionNotAllowed` if the
    /// current executor is not the owner of record.
    ///
    /// # Errors
    /// Returns `NotFound` if `k` is absent, `ActionNotAllowed` if the current
    /// executor is not the stored owner, or any underlying storage error.
    pub fn update(&mut self, k: &K, v: V) -> Result<(), StoreError> {
        let entry_id = self.entry_id(k);
        let stored_owner = self
            .owner_of(k)?
            .ok_or(StoreError::StorageError(StorageError::NotFound(entry_id)))?;

        let executor: PublicKey = env::executor_id().into();
        if stored_owner != executor {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "AuthoredMap::update: not entry owner".to_owned(),
            )));
        }

        // Mutate V in place via the map's guard. The entry's Element — and
        // therefore its `StorageType::User { owner }` stamp — is preserved;
        // only `updated_at` advances, which the save path uses as the signing
        // nonce for the local user action.
        let mut guard = self
            .inner
            .get_mut(k)?
            .ok_or(StoreError::StorageError(StorageError::NotFound(entry_id)))?;
        *guard = v;
        Ok(())
    }

    /// Removes `k`. Only the entry's owner may call this.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if the current executor is not the stored
    /// owner, or any underlying storage error. Returns `Ok(None)` if `k` is
    /// absent.
    pub fn remove(&mut self, k: &K) -> Result<Option<V>, StoreError> {
        let Some(stored_owner) = self.owner_of(k)? else {
            return Ok(None);
        };

        let executor: PublicKey = env::executor_id().into();
        if stored_owner != executor {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "AuthoredMap::remove: not entry owner".to_owned(),
            )));
        }

        self.inner.remove(k)
    }

    /// Returns the value at `k`, if any.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn get(&self, k: &K) -> Result<Option<V>, StoreError> {
        self.inner.get(k)
    }

    /// Returns whether `k` is present.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn contains(&self, k: &K) -> Result<bool, StoreError> {
        self.inner.contains(k)
    }

    /// Returns the public key of the owner of `k`, if any.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn owner_of(&self, k: &K) -> Result<Option<PublicKey>, StoreError> {
        let id = self.entry_id(k);
        let metadata = <Index<S>>::get_metadata(id).map_err(StoreError::StorageError)?;
        Ok(metadata.and_then(|m| match m.storage_type {
            StorageType::User { owner, .. } => Some(owner),
            _ => None,
        }))
    }

    /// Iterates over all `(k, v)` entries.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn entries(&self) -> Result<impl Iterator<Item = (K, V)> + '_, StoreError> {
        self.inner.entries()
    }

    /// Returns the number of entries.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.inner.len()
    }

    pub(crate) fn entry_id(&self, k: &K) -> crate::address::Id {
        compute_id(self.inner.element().id(), k.as_ref())
    }
}

impl<K, V, S> Data for AuthoredMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        self.inner.collections()
    }

    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }
}

impl<K, V, S> Mergeable for AuthoredMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + Clone + PartialEq,
    V: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    /// `AuthoredMap` deliberately does **not** perform structural merge.
    ///
    /// Per-entry merge is handled by `Interface::apply_action` on the signed
    /// delta path: each remote upsert/delete carries a `StorageType::User`
    /// metadata block and is verified against the entry's stored owner.
    /// At the container level, `CrdtType::UserStorage` dispatches to the
    /// byte-level path in `merge.rs` (`incoming.to_vec()`), so this
    /// `Mergeable::merge` impl is not reached at runtime in the normal flow.
    ///
    /// Delegating to `UnorderedMap::merge` here would be **unsafe**: for any
    /// key present only in `other`, it would call `UnorderedMap::insert`
    /// which stamps `StorageType::Public`, silently stripping ownership.
    /// We therefore make this a no-op rather than delegate.
    fn merge(&mut self, _other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
        Ok(())
    }
}

impl<K, V, S> CrdtMeta for AuthoredMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn crdt_type() -> CrdtType {
        CrdtType::UserStorage
    }
    fn storage_strategy() -> StorageStrategy {
        StorageStrategy::Structured
    }
    fn can_contain_crdts() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use calimero_primitives::identity::PublicKey;
    use serial_test::serial;

    use super::AuthoredMap;
    use crate::collections::Root;
    use crate::env;

    const ALICE: [u8; 32] = [0x11; 32];
    const BOB: [u8; 32] = [0x22; 32];

    fn pk(bytes: [u8; 32]) -> PublicKey {
        bytes.into()
    }

    #[test]
    #[serial]
    fn insert_stamps_current_executor_as_owner() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut map = Root::new(|| AuthoredMap::<String, u64>::new());
        map.insert("apple".to_owned(), 1).expect("insert");

        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(1));
        assert_eq!(map.owner_of(&"apple".to_owned()).unwrap(), Some(pk(ALICE)));
        assert_eq!(map.len().unwrap(), 1);
    }

    #[test]
    #[serial]
    fn insert_rejects_existing_key() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut map = Root::new(|| AuthoredMap::<String, u64>::new());
        map.insert("apple".to_owned(), 1).unwrap();

        let err = map
            .insert("apple".to_owned(), 2)
            .expect_err("insert on existing key must fail");
        assert!(
            err.to_string().to_lowercase().contains("already"),
            "error should mention key already exists, got: {err}"
        );
        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(1));
    }

    #[test]
    #[serial]
    fn update_by_owner_succeeds() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut map = Root::new(|| AuthoredMap::<String, u64>::new());
        map.insert("apple".to_owned(), 1).unwrap();
        map.update(&"apple".to_owned(), 42).expect("owner update");

        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(42));
        assert_eq!(map.owner_of(&"apple".to_owned()).unwrap(), Some(pk(ALICE)));
    }

    #[test]
    #[serial]
    fn update_by_non_owner_rejected() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut map = Root::new(|| AuthoredMap::<String, u64>::new());
        map.insert("apple".to_owned(), 1).unwrap();

        env::set_executor_id(BOB);
        let err = map
            .update(&"apple".to_owned(), 99)
            .expect_err("non-owner update must fail");
        assert!(
            err.to_string().to_lowercase().contains("owner"),
            "error should mention ownership, got: {err}"
        );
        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(1));
        assert_eq!(map.owner_of(&"apple".to_owned()).unwrap(), Some(pk(ALICE)));
    }

    #[test]
    #[serial]
    fn update_missing_key_errors() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut map = Root::new(|| AuthoredMap::<String, u64>::new());
        let err = map
            .update(&"ghost".to_owned(), 1)
            .expect_err("missing key update must fail");
        assert!(
            err.to_string().to_lowercase().contains("not found")
                || err.to_string().to_lowercase().contains("record"),
            "error should indicate missing key, got: {err}"
        );
    }

    #[test]
    #[serial]
    fn remove_by_owner_succeeds() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut map = Root::new(|| AuthoredMap::<String, u64>::new());
        map.insert("apple".to_owned(), 1).unwrap();

        let removed = map.remove(&"apple".to_owned()).unwrap();
        assert_eq!(removed, Some(1));
        assert_eq!(map.get(&"apple".to_owned()).unwrap(), None);
        assert_eq!(map.len().unwrap(), 0);
    }

    #[test]
    #[serial]
    fn remove_by_non_owner_rejected() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut map = Root::new(|| AuthoredMap::<String, u64>::new());
        map.insert("apple".to_owned(), 1).unwrap();

        env::set_executor_id(BOB);
        let err = map
            .remove(&"apple".to_owned())
            .expect_err("non-owner remove must fail");
        assert!(err.to_string().to_lowercase().contains("owner"));
        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(1));
    }

    #[test]
    #[serial]
    fn remove_missing_key_is_none() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut map = Root::new(|| AuthoredMap::<String, u64>::new());
        assert_eq!(map.remove(&"ghost".to_owned()).unwrap(), None);
    }

    #[test]
    #[serial]
    fn different_users_own_disjoint_keys_in_shared_keyspace() {
        env::reset_for_testing();

        let mut map = Root::new(|| AuthoredMap::<String, u64>::new());

        env::set_executor_id(ALICE);
        map.insert("alice_key".to_owned(), 1).unwrap();

        env::set_executor_id(BOB);
        map.insert("bob_key".to_owned(), 2).unwrap();

        assert_eq!(map.len().unwrap(), 2);
        assert_eq!(
            map.owner_of(&"alice_key".to_owned()).unwrap(),
            Some(pk(ALICE))
        );
        assert_eq!(map.owner_of(&"bob_key".to_owned()).unwrap(), Some(pk(BOB)));

        // Bob cannot overwrite Alice's key via insert (it already exists).
        let err = map.insert("alice_key".to_owned(), 99);
        assert!(err.is_err());

        // Bob cannot update or remove Alice's key.
        assert!(map.update(&"alice_key".to_owned(), 99).is_err());
        assert!(map.remove(&"alice_key".to_owned()).is_err());

        // Alice still sees her original value.
        env::set_executor_id(ALICE);
        assert_eq!(map.get(&"alice_key".to_owned()).unwrap(), Some(1));
    }

    #[test]
    #[serial]
    fn owner_of_missing_key_is_none() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let map = Root::new(|| AuthoredMap::<String, u64>::new());
        assert_eq!(map.owner_of(&"ghost".to_owned()).unwrap(), None);
    }

    #[test]
    #[serial]
    fn entries_contains_all_inserted_pairs() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut map = Root::new(|| AuthoredMap::<String, u64>::new());
        map.insert("a".to_owned(), 1).unwrap();
        map.insert("b".to_owned(), 2).unwrap();
        env::set_executor_id(BOB);
        map.insert("c".to_owned(), 3).unwrap();

        let pairs: Vec<_> = map.entries().unwrap().collect();
        assert_eq!(pairs.len(), 3);
        assert!(pairs.contains(&("a".to_owned(), 1)));
        assert!(pairs.contains(&("b".to_owned(), 2)));
        assert!(pairs.contains(&("c".to_owned(), 3)));
    }
}
