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
//!
//! # Merge semantics
//!
//! `AuthoredMap` implements [`Mergeable`](super::crdt_meta::Mergeable) by
//! delegating to its inner `UnorderedMap`. The owner stamp travels with each
//! entry, so per-entry authorization survives the merge: post-merge,
//! `update`/`remove` still reject calls from non-owners. New keys from either
//! side are unioned; updates to the same key on both sides resolve through the
//! inner map's merge (typically LWW on the contained value).

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::PublicKey;

use super::crdt_meta::{CrdtMeta, CrdtType, Mergeable, StorageStrategy};
use super::{compute_id, StoreError, UnorderedMap, ValueRef};
use crate::entities::{ChildInfo, Data, Element, StorageType};
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
        K: AsRef<[u8]> + PartialEq + 'static,
        V: 'static,
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
    pub fn insert(&mut self, k: K, v: V) -> Result<(), StoreError>
    where
        K: 'static,
        V: 'static,
    {
        if self.inner.contains(&k)? {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "AuthoredMap::insert: key already exists".to_owned(),
            )));
        }

        let storage_type = super::authored_common::make_owner_stamp();

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

        if !super::authored_common::executor_matches_owner(&stored_owner) {
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

        if !super::authored_common::executor_matches_owner(&stored_owner) {
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
        Ok(self.inner.get(k)?.map(ValueRef::into_inner))
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

    /// Returns `true` if there are no entries.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }

    /// Returns the entry's stamped `schema_version`, or `None` if the key is
    /// absent or the entry was never stamped (legacy). Reads the Merkle-invisible
    /// `Metadata.schema_version`; used to skip already-migrated entries.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn entry_schema_version(&self, k: &K) -> Result<Option<u32>, StoreError> {
        let id = self.entry_id(k);
        let metadata = <Index<S>>::get_metadata(id).map_err(StoreError::StorageError)?;
        Ok(metadata.and_then(|m| m.schema_version))
    }

    /// Returns whether the current executor owns `k`. False for absent keys.
    /// Only the owner can drive the per-entry convert, so this gates which
    /// entries `migrate_my_entries()` re-writes.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn owned_by_me(&self, k: &K) -> Result<bool, StoreError> {
        Ok(self
            .owner_of(k)?
            .as_ref()
            .is_some_and(super::authored_common::executor_matches_owner))
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

// #D5: RekeyTarget supertrait — delegate to the inner collection.
impl<K, V, S> crate::collections::rekey::RekeyTarget for AuthoredMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq + 'static,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
        self.inner.rekey_relative_to(parent_id);
    }
}

#[diagnostic::do_not_recommend]
impl<K, V, S> Mergeable for AuthoredMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + Clone + PartialEq + 'static,
    V: BorshSerialize + BorshDeserialize + Mergeable + 'static,
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

    #[test]
    fn test_new_plus_reassign_is_convergent() {
        // Wrapper type: `new_with_field_name` leaves the wrapper id random and
        // only the inner map deterministic; `reassign` canonicalises the wrapper
        // too. The CIP-I9 property is convergence — two independent
        // `new() + reassign("f")` mint the same id (stronger than matching
        // `new_with_field_name`). Inner-map determinism is covered by the
        // `UnorderedMap` tests.
        crate::env::reset_for_testing();
        let mut a: AuthoredMap<String, u32> = AuthoredMap::new();
        a.reassign_deterministic_id("items");
        let mut b: AuthoredMap<String, u32> = AuthoredMap::new();
        b.reassign_deterministic_id("items");
        assert_eq!(
            <AuthoredMap<String, u32> as crate::entities::Data>::id(&a),
            <AuthoredMap<String, u32> as crate::entities::Data>::id(&b),
        );
    }

    fn pk(bytes: [u8; 32]) -> PublicKey {
        bytes.into()
    }

    #[test]
    #[serial]
    fn insert_stamps_current_executor_as_owner() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
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

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
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

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
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

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
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

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
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

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
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

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
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

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
        assert_eq!(map.remove(&"ghost".to_owned()).unwrap(), None);
    }

    #[test]
    #[serial]
    fn different_users_own_disjoint_keys_in_shared_keyspace() {
        env::reset_for_testing();

        let mut map = Root::new(AuthoredMap::<String, u64>::new);

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

        let map = Root::new(AuthoredMap::<String, u64>::new);
        assert_eq!(map.owner_of(&"ghost".to_owned()).unwrap(), None);
    }

    // Reproduction for the migrate_my_entries convert path: an owner re-write
    // via the REAL `update()` (the call migrate_my_entries makes) must PERSIST
    // the schema re-stamp. The existing owner_driven_convert tests hand a
    // bumped nonce straight to save_raw and never exercise update(); this pins
    // that the guard write actually advances the nonce enough for save_internal
    // to persist, otherwise migrate_my_entries reports `converted` while the
    // entry silently stays stale (no re-stamp, no node-log).
    #[test]
    #[serial]
    fn owner_update_persists_schema_restamp() {
        use calimero_sdk::event::NoEvent;
        use calimero_sdk::state::{AppState, AppStateInit};

        #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
        struct V2;
        impl AppStateInit for V2 {
            type Return = V2;
        }
        impl AppState for V2 {
            type Event<'a> = NoEvent;
            const SCHEMA_VERSION: u32 = 2;
        }
        #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
        struct Unversioned;
        impl AppStateInit for Unversioned {
            type Return = Unversioned;
        }
        impl AppState for Unversioned {
            type Event<'a> = NoEvent;
        }

        env::reset_for_testing();
        env::set_executor_id(ALICE);

        // Insert at the default (unversioned 0) target — the "v1" stamp.
        let mut map = Root::new(AuthoredMap::<String, u64>::new);
        map.insert("k".to_owned(), 1).unwrap();
        assert_eq!(map.entry_schema_version(&"k".to_owned()).unwrap(), Some(0));

        // The binary is now v2.
        calimero_sdk::app::register_schema_version::<V2>();

        // The one-tap convert: owner re-writes through the SAME update() path
        // migrate_my_entries uses (value unchanged).
        map.update(&"k".to_owned(), 1).unwrap();

        // The re-stamp MUST have persisted.
        let after = map.entry_schema_version(&"k".to_owned()).unwrap();
        calimero_sdk::app::register_schema_version::<Unversioned>(); // reset global
        assert_eq!(
            after,
            Some(2),
            "owner update() must persist the schema re-stamp; got {after:?}"
        );
    }

    // Same as above but with the EXACT value type + read-then-write pattern the
    // scenario-32 fixture (and migrate_my_entries) use: AuthoredMap<_, LwwRegister>
    // re-written with the value read back from `get()`. An LwwRegister carries its
    // own HLC; if the entry nonce is taken from the (stale) register instead of a
    // fresh write nonce, save_internal's LWW gate drops the convert and the
    // re-stamp never persists.
    #[test]
    #[serial]
    fn owner_update_persists_schema_restamp_lww_readback() {
        use calimero_sdk::event::NoEvent;
        use calimero_sdk::state::{AppState, AppStateInit};

        use crate::collections::LwwRegister;

        #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
        struct V2;
        impl AppStateInit for V2 {
            type Return = V2;
        }
        impl AppState for V2 {
            type Event<'a> = NoEvent;
            const SCHEMA_VERSION: u32 = 2;
        }
        #[derive(borsh::BorshSerialize, borsh::BorshDeserialize)]
        struct Unversioned;
        impl AppStateInit for Unversioned {
            type Return = Unversioned;
        }
        impl AppState for Unversioned {
            type Event<'a> = NoEvent;
        }

        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut map = Root::new(AuthoredMap::<String, LwwRegister<String>>::new);
        map.insert("k".to_owned(), LwwRegister::new("v1".to_owned()))
            .unwrap();
        assert_eq!(map.entry_schema_version(&"k".to_owned()).unwrap(), Some(0));

        calimero_sdk::app::register_schema_version::<V2>();

        // Mirror migrate_my_entries exactly: read the value, then write it back.
        let v = map.get(&"k".to_owned()).unwrap().expect("entry present");
        map.update(&"k".to_owned(), v).unwrap();

        let after = map.entry_schema_version(&"k".to_owned()).unwrap();
        calimero_sdk::app::register_schema_version::<Unversioned>();
        assert_eq!(
            after,
            Some(2),
            "owner update() of an LwwRegister read-back must persist the re-stamp; got {after:?}"
        );
    }

    #[test]
    #[serial]
    fn entry_schema_version_and_ownership_reflect_stored_metadata() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
        map.insert("apple".to_owned(), 1).unwrap();

        // An owner write stamps the binary's current target schema version
        // (0 in the unit env, where no app is registered).
        assert_eq!(
            map.entry_schema_version(&"apple".to_owned()).unwrap(),
            Some(calimero_sdk::app::schema_version()),
        );
        assert!(map.owned_by_me(&"apple".to_owned()).unwrap());

        // A different executor is not the owner.
        env::set_executor_id(BOB);
        assert!(!map.owned_by_me(&"apple".to_owned()).unwrap());

        // Absent key: no version, not owned.
        env::set_executor_id(ALICE);
        assert_eq!(map.entry_schema_version(&"ghost".to_owned()).unwrap(), None);
        assert!(!map.owned_by_me(&"ghost".to_owned()).unwrap());
    }

    #[test]
    #[serial]
    fn entries_contains_all_inserted_pairs() {
        env::reset_for_testing();
        env::set_executor_id(ALICE);

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
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

    /// `AuthoredMap` is now matched by the `#[app::state]` macro's
    /// `is_collection_type`, so `__assign_deterministic_ids()` calls
    /// `reassign_deterministic_id` on it. This must NOT strip owner stamps or
    /// drop entries: the inner map is built with a deterministic id, so its
    /// `reassign` is an idempotent no-op (no clear+reinsert) and only the outer
    /// wrapper id is canonicalised. This guards the macro change as non-breaking
    /// for existing maps carried through a migration.
    #[test]
    #[serial]
    fn reassign_deterministic_id_preserves_entries_and_owners() {
        env::reset_for_testing();

        let mut map = Root::new(|| AuthoredMap::<String, u64>::new_with_field_name("entries"));
        env::set_executor_id(ALICE);
        map.insert("apple".to_owned(), 1).expect("alice insert");
        env::set_executor_id(BOB);
        map.insert("banana".to_owned(), 2).expect("bob insert");

        // Simulate the macro-driven id canonicalisation.
        map.reassign_deterministic_id("entries");

        // Values survive.
        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(1));
        assert_eq!(map.get(&"banana".to_owned()).unwrap(), Some(2));
        assert_eq!(map.len().unwrap(), 2);
        // Owner stamps survive (not re-stamped to the calling executor).
        assert_eq!(map.owner_of(&"apple".to_owned()).unwrap(), Some(pk(ALICE)));
        assert_eq!(map.owner_of(&"banana".to_owned()).unwrap(), Some(pk(BOB)));

        // Idempotent: a second reassign is a no-op and still preserves everything.
        map.reassign_deterministic_id("entries");
        assert_eq!(map.owner_of(&"apple".to_owned()).unwrap(), Some(pk(ALICE)));
        assert_eq!(map.owner_of(&"banana".to_owned()).unwrap(), Some(pk(BOB)));
        assert_eq!(map.len().unwrap(), 2);
    }

    /// Non-vacuous counterpart: building with `new()` (random inner id) forces
    /// the reassign down the clear+reinsert path — the one that used to drop
    /// per-entry `StorageType` and downgrade authored entries to `Public`. The
    /// owner stamps must survive that path.
    #[test]
    #[serial]
    fn reassign_clear_reinsert_path_preserves_owner_stamps() {
        env::reset_for_testing();

        let mut map = Root::new(AuthoredMap::<String, u64>::new);
        env::set_executor_id(ALICE);
        map.insert("apple".to_owned(), 1).expect("alice insert");
        env::set_executor_id(BOB);
        map.insert("banana".to_owned(), 2).expect("bob insert");

        // Random inner id != deterministic id => real clear+reinsert (not the
        // no-op fast path the sibling test exercises).
        map.reassign_deterministic_id("entries");

        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(1));
        assert_eq!(map.get(&"banana".to_owned()).unwrap(), Some(2));
        assert_eq!(map.len().unwrap(), 2);
        assert_eq!(map.owner_of(&"apple".to_owned()).unwrap(), Some(pk(ALICE)));
        assert_eq!(map.owner_of(&"banana".to_owned()).unwrap(), Some(pk(BOB)));
    }
}
