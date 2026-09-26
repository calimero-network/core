//! Ordered shared-keyspace map with per-entry ownership.
//!
//! [`AuthoredSortedMap<K, V>`] is to [`AuthoredMap`](super::authored_map::AuthoredMap)
//! what [`SortedMap`](super::SortedMap) is to [`UnorderedMap`](super::UnorderedMap):
//! the same entries, the same owner stamp, the same merge — plus an ordered
//! view, so a reader can seek to a range or a key prefix instead of walking
//! the whole collection.
//!
//! # Why this exists
//!
//! `AuthoredMap` has no ordered read at all: `entries()` is the only iteration
//! and it loads every entry in the collection. For an app whose keys are
//! hierarchical — `"<game>/<ply>/<account>/<nonce>"`, `"<room>/<ts>/<author>"`,
//! `"<doc>/<section>/<editor>"` — that turns "read one slice" into "read
//! everything", and the slice is usually a rounding error next to the whole.
//!
//! That is a **liveness** problem specific to authored data, not just a
//! performance one, and the two halves compound:
//!
//! * Any context member may INSERT under any key. Insert is open by design —
//!   that is what makes the keyspace shared.
//! * Only an entry's own owner may ever REMOVE it. So entries written by
//!   somebody acting in bad faith cannot be cleaned up by anyone else, ever.
//!
//! Put together: a member can grow the collection without bound, nobody can
//! shrink it, and with only a full scan available every honest reader pays for
//! every junk entry on every read, forever. The entries need not be *believed*
//! to do damage — an app that correctly ignores all of them still reads them
//! all. Correct answers, unusable app.
//!
//! An ordered index turns that back into `O(log n + k)`: a reader pays for the
//! `k` entries under the prefix it asked for and nothing for the rest. A
//! targeted writer can still crowd one specific prefix, which no collection can
//! prevent, but the blast radius of an untargeted flood drops to zero.
//!
//! # What it costs
//!
//! Exactly what `SortedMap` costs, and for the same reason: a node-local,
//! derived, **non-synced** secondary index, maintained on every `insert` /
//! `remove` and rebuilt once after a remote sync (which mutates entries
//! host-side without going through those methods). So:
//!
//! * writes pay an extra index write and a marker read/write,
//! * the node stores a little more per key,
//! * the first ordered read after a sync is `O(n)`.
//!
//! **Default to [`AuthoredMap`](super::authored_map::AuthoredMap).** Reach for
//! this one when the keys are hierarchical and reads are slices of them — which
//! is also exactly when the flood above is worth defending against.
//!
//! # Merge semantics
//!
//! Identical to `AuthoredMap`, and deliberately indistinguishable on the wire:
//! the ordering is derived from the key set by each node for itself, so there
//! is nothing extra to replicate. Entries carry `StorageType::User { owner }`,
//! per-entry authorization is enforced at apply in `Interface::apply_action`,
//! and the container reports [`CrdtType::UserStorage`] — the same variant
//! `AuthoredMap` reports, on purpose. A new `CrdtType` would have been a borsh
//! discriminant change for a distinction the sync layer must not make: two
//! nodes holding the same entries, one using each collection, have to agree on
//! the root hash, and they do.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::RangeBounds;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;

use super::crdt_meta::{CrdtMeta, CrdtType, MergeStrategy, Mergeable, StorageStrategy};
use super::{compute_id, SortedMap, StorageKey, StoreError, ValueRef};
use crate::entities::{ChildInfo, Data, Element, StorageType};
use crate::index::Index;
use crate::interface::StorageError;
use crate::store::{MainStorage, StorageAdaptor};

/// A map keyed by `K`, iterated in ascending key order, where each entry is
/// owned by the account that inserted it.
///
/// Internally a [`SortedMap<K, V>`](super::SortedMap). Each entry's
/// `StorageType` is `User { owner }`, set at insert time from
/// `env::account_id()`. Only that account can `update` or `remove` the entry,
/// from any device it holds. Reads are unrestricted.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct AuthoredSortedMap<K, V, S: StorageAdaptor = MainStorage>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
{
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: SortedMap<K, V, S>,
    storage: Element,
}

impl<K, V, S> AuthoredSortedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Creates a new, empty `AuthoredSortedMap` with a random ID.
    ///
    /// Use this for nested collections. For top-level state fields, use
    /// [`new_with_field_name`](Self::new_with_field_name).
    pub fn new() -> Self {
        Self {
            inner: <SortedMap<K, V, S>>::new(),
            storage: Element::new(None),
        }
    }

    /// Creates a new, empty `AuthoredSortedMap` with a deterministic ID derived
    /// from `field_name`.
    pub fn new_with_field_name(field_name: &str) -> Self {
        let mut storage = Element::new_with_field_name(None, Some(field_name.to_string()));
        storage.metadata.crdt_type = Some(CrdtType::UserStorage);
        Self {
            inner: <SortedMap<K, V, S>>::new_with_field_name(&format!(
                "__authored_sorted_map_{field_name}"
            )),
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
            .reassign_deterministic_id(&format!("__authored_sorted_map_{field_name}"));
    }
}

// Hand-written rather than derived: the inner `SortedMap`'s own `Debug` needs
// `K: Ord` (it prints in key order), and deriving here would propagate that
// bound onto the struct itself, where nothing else wants it.
impl<K, V, S> fmt::Debug for AuthoredSortedMap<K, V, S>
where
    K: Ord + fmt::Debug + BorshSerialize + BorshDeserialize,
    V: fmt::Debug + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthoredSortedMap")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<K, V, S> Default for AuthoredSortedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V, S> AuthoredSortedMap<K, V, S>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    /// Inserts a new entry, stamping the current executor's account as its
    /// owner.
    ///
    /// Fails with `ActionNotAllowed` if `k` is already present — ownership is
    /// not transferable, so an occupied key is never quietly taken over. Use
    /// `remove` + `insert` from the new owner.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if `k` already exists, or any underlying
    /// storage error.
    pub fn insert(&mut self, k: K, v: V) -> Result<(), StoreError> {
        if self.inner.contains(&k)? {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "AuthoredSortedMap::insert: key already exists".to_owned(),
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
    /// # Errors
    /// Returns `NotFound` if `k` is absent, `ActionNotAllowed` if the current
    /// executor is not the stored owner, or any underlying storage error.
    pub fn update(&mut self, k: &K, v: V) -> Result<(), StoreError> {
        let entry_id = self.entry_id(k);
        let stored_owner = self
            .owner_of(k)?
            .ok_or(StoreError::StorageError(StorageError::NotFound(entry_id)))?;

        if !super::authored_common::writer_matches_owner(&stored_owner) {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "AuthoredSortedMap::update: not entry owner".to_owned(),
            )));
        }

        // Mutate V in place via the map's guard. The entry's Element — and
        // therefore its `StorageType::User { owner }` stamp — is preserved;
        // only `updated_at` advances, which the save path uses as the signing
        // nonce for the local user action. The key set is untouched, so the
        // ordered index needs no maintenance here.
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

        if !super::authored_common::writer_matches_owner(&stored_owner) {
            return Err(StoreError::StorageError(StorageError::ActionNotAllowed(
                "AuthoredSortedMap::remove: not entry owner".to_owned(),
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

    /// Returns the account that owns `k`, if any.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn owner_of(&self, k: &K) -> Result<Option<AccountId>, StoreError> {
        let id = self.entry_id(k);
        let metadata = <Index<S>>::get_metadata(id).map_err(StoreError::StorageError)?;
        Ok(metadata.and_then(|m| match m.storage_type {
            StorageType::User { owner, .. } => Some(owner),
            _ => None,
        }))
    }

    /// Iterates every `(k, v)` entry, in ascending key order.
    ///
    /// `O(n)`: it returns everything. Prefer [`prefix`](Self::prefix) or
    /// [`range`](Self::range) — the reason to hold this collection rather than
    /// an [`AuthoredMap`](super::authored_map::AuthoredMap) is to not do this.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn entries(&self) -> Result<impl Iterator<Item = (K, V)>, StoreError>
    where
        K: Ord,
    {
        self.inner.entries()
    }

    /// Iterates the keys, in ascending order, without loading any values.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn keys(&self) -> Result<impl Iterator<Item = K>, StoreError>
    where
        K: Ord,
    {
        self.inner.keys()
    }

    /// Iterates the entries whose key bytes start with `prefix`, in ascending
    /// order — `O(log n + k)` where `k` is the number of matches.
    ///
    /// This is the method this collection exists for. Only the matching
    /// entries' values are loaded, so entries outside the prefix — including
    /// any a hostile member has piled up elsewhere in the keyspace — cost
    /// nothing to skip.
    ///
    /// The prefix is matched on `K`'s bytes (`AsRef<[u8]>`), which for the
    /// hierarchical `String` keys this is meant for is the obvious thing. Note
    /// that a prefix is a claim about the KEY only: it says nothing about who
    /// owns the entries it returns, so a caller that cares must still check
    /// [`owner_of`](Self::owner_of) (or put the author in the key and prefix on
    /// that, which makes a mismatch cheap to spot).
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn prefix(&self, prefix: &[u8]) -> Result<impl Iterator<Item = (K, V)>, StoreError>
    where
        K: Ord,
    {
        self.inner.prefix(prefix)
    }

    /// Iterates the entries whose keys fall in `range`, in ascending order —
    /// `O(log n + k)`.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn range<R>(&self, range: R) -> Result<impl Iterator<Item = (K, V)>, StoreError>
    where
        R: RangeBounds<K>,
        K: Ord,
    {
        self.inner.range(range)
    }

    /// Returns a page of `limit` entries starting at `offset`, in ascending key
    /// order — `O(offset + limit)`.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn page(&self, offset: usize, limit: usize) -> Result<Vec<(K, V)>, StoreError>
    where
        K: Ord,
    {
        self.inner.page(offset, limit)
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
    /// Returns any underlying storage error.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }

    /// Returns the entry's stamped `schema_version`, or `None` if the key is
    /// absent or the entry was never stamped (legacy). Reads the
    /// Merkle-invisible `Metadata.schema_version`; used to skip already-migrated
    /// entries.
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
            .is_some_and(super::authored_common::writer_matches_owner))
    }

    pub(crate) fn entry_id(&self, k: &K) -> crate::address::Id {
        compute_id(
            <SortedMap<K, V, S> as Data>::element(&self.inner).id(),
            k.as_ref(),
        )
    }
}

impl<K, V, S> Data for AuthoredSortedMap<K, V, S>
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

// RekeyTarget supertrait — delegate to the inner collection.
impl<K, V, S> crate::collections::rekey::RekeyTarget for AuthoredSortedMap<K, V, S>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    fn rekey_relative_to(&mut self, parent_id: crate::address::Id) {
        self.inner.rekey_relative_to(parent_id);
    }
}

#[diagnostic::do_not_recommend]
impl<K, V, S> Mergeable for AuthoredSortedMap<K, V, S>
where
    K: StorageKey + Clone,
    V: BorshSerialize + BorshDeserialize + Mergeable + 'static,
    S: StorageAdaptor,
{
    /// A no-op, for the same reason
    /// [`AuthoredMap`](super::authored_map::AuthoredMap)'s is.
    ///
    /// Per-entry merge happens in `Interface::apply_action` on the signed delta
    /// path, where each upsert/delete carries a `StorageType::User` block that
    /// is verified against the entry's stored owner. At the container level
    /// `CrdtType::UserStorage` dispatches to the byte-level path in `merge.rs`,
    /// so this is not reached in the normal flow.
    ///
    /// Delegating to `SortedMap::merge` would be **unsafe**: for a key present
    /// only in `other` it would insert with the collection's inherited storage
    /// type, silently stripping ownership.
    fn merge(&mut self, _other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
        Ok(())
    }
}

impl<K, V, S> CrdtMeta for AuthoredSortedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// `UserStorage`, the same variant
    /// [`AuthoredMap`](super::authored_map::AuthoredMap) reports — see the
    /// module docs. The ordering is a node-local derived index; there is
    /// nothing about it for the sync layer to classify differently.
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

/// Structural: the storage layer merges this by its `crdt_type` variant, so
/// there is no app rule to dispatch. See [`MergeStrategy`].
#[diagnostic::do_not_recommend]
impl<K, V, S> MergeStrategy for AuthoredSortedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    const DISPATCHED: bool = false;
}

#[cfg(test)]
mod tests {
    use calimero_account::AccountId;
    use serial_test::serial;

    use super::AuthoredSortedMap;
    use crate::collections::Root;
    use crate::env;

    const ALICE: [u8; 32] = [0x11; 32];
    const BOB: [u8; 32] = [0x22; 32];

    /// The tests name the OWNER, and an owner is an account.
    fn acct(bytes: [u8; 32]) -> AccountId {
        AccountId::from(bytes)
    }

    fn map() -> Root<AuthoredSortedMap<String, u64>> {
        Root::new(AuthoredSortedMap::<String, u64>::new)
    }

    #[test]
    fn new_plus_reassign_is_convergent() {
        // Wrapper type: `new_with_field_name` leaves the wrapper id random and
        // only the inner map deterministic; `reassign` canonicalises the
        // wrapper too. The CIP-I9 property is convergence — two independent
        // `new() + reassign("f")` mint the same id.
        env::reset_for_testing();
        let mut a: AuthoredSortedMap<String, u32> = AuthoredSortedMap::new();
        a.reassign_deterministic_id("items");
        let mut b: AuthoredSortedMap<String, u32> = AuthoredSortedMap::new();
        b.reassign_deterministic_id("items");
        assert_eq!(
            <AuthoredSortedMap<String, u32> as crate::entities::Data>::id(&a),
            <AuthoredSortedMap<String, u32> as crate::entities::Data>::id(&b),
        );
    }

    // ── the owner gate, which must behave exactly as AuthoredMap's ──────────

    #[test]
    #[serial]
    fn insert_stamps_current_executor_as_owner() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = map();
        map.insert("apple".to_owned(), 1).expect("insert");

        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(1));
        assert_eq!(
            map.owner_of(&"apple".to_owned()).unwrap(),
            Some(acct(ALICE))
        );
        assert_eq!(map.len().unwrap(), 1);
    }

    #[test]
    #[serial]
    fn insert_rejects_existing_key() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = map();
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
    fn a_second_account_cannot_take_an_occupied_key() {
        // The squat case the other way round: BOB cannot claim ALICE's key by
        // inserting over it, and cannot update or remove it either. Insert is
        // open; an OCCUPIED key is not.
        env::reset_for_testing();
        env::set_account_id(ALICE);
        let mut map = map();
        map.insert("apple".to_owned(), 1).unwrap();

        env::set_account_id(BOB);
        assert!(map.insert("apple".to_owned(), 2).is_err());
        assert!(map.update(&"apple".to_owned(), 99).is_err());
        assert!(map.remove(&"apple".to_owned()).is_err());

        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(1));
        assert_eq!(
            map.owner_of(&"apple".to_owned()).unwrap(),
            Some(acct(ALICE))
        );
    }

    #[test]
    #[serial]
    fn update_and_remove_by_owner_succeed() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = map();
        map.insert("apple".to_owned(), 1).unwrap();
        map.update(&"apple".to_owned(), 42).expect("owner update");
        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(42));
        // The stamp survives an update — an owner editing their own entry must
        // not look like a fresh insert by whoever happens to be executing.
        assert_eq!(
            map.owner_of(&"apple".to_owned()).unwrap(),
            Some(acct(ALICE))
        );

        assert_eq!(map.remove(&"apple".to_owned()).unwrap(), Some(42));
        assert_eq!(map.get(&"apple".to_owned()).unwrap(), None);
    }

    #[test]
    #[serial]
    fn a_second_device_of_the_owning_account_may_still_write() {
        // Authorship is the ACCOUNT, not the device: a laptop editing what the
        // phone wrote is the same person.
        env::reset_for_testing();
        env::set_account_id(ALICE);
        env::set_device_id([0xD1; 32]);
        let mut map = map();
        map.insert("apple".to_owned(), 1).unwrap();

        env::set_device_id([0xD2; 32]);
        map.update(&"apple".to_owned(), 7)
            .expect("a second device of the owner must be able to write");
        assert_eq!(map.get(&"apple".to_owned()).unwrap(), Some(7));
    }

    // ── the ordered reads, which are the reason this type exists ────────────

    #[test]
    #[serial]
    fn entries_and_keys_come_back_in_key_order() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = map();
        for k in ["c", "a", "b"] {
            map.insert(k.to_owned(), 1).unwrap();
        }

        let keys: Vec<String> = map.keys().unwrap().collect();
        assert_eq!(keys, vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]);
    }

    #[test]
    #[serial]
    fn prefix_returns_only_the_slice_asked_for() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = map();
        for k in [
            "game/0000/alice/1",
            "game/0000/alice/2",
            "game/0001/bob/1",
            "other/0000/alice/1",
        ] {
            map.insert(k.to_owned(), 1).unwrap();
        }

        let hits: Vec<String> = map.prefix(b"game/0000/").unwrap().map(|(k, _)| k).collect();
        assert_eq!(
            hits,
            vec![
                "game/0000/alice/1".to_owned(),
                "game/0000/alice/2".to_owned()
            ]
        );

        // A prefix that narrows to one author is the shape the owner gate makes
        // worth having: the entries it returns are the only ones whose stamp
        // then has to be checked.
        let bobs: Vec<String> = map
            .prefix(b"game/0001/bob/")
            .unwrap()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(bobs, vec!["game/0001/bob/1".to_owned()]);

        assert!(map.prefix(b"nothing/").unwrap().next().is_none());
    }

    #[test]
    #[serial]
    fn a_prefix_read_is_unmoved_by_entries_outside_it() {
        // The property the collection exists for, stated as behaviour rather
        // than as a timing: entries piled up elsewhere in the keyspace — which
        // any member may write and only their own author may ever remove —
        // do not appear in, and do not enlarge, another prefix's result.
        env::reset_for_testing();
        env::set_account_id(ALICE);
        let mut map = map();
        map.insert("game/0000/alice/1".to_owned(), 1).unwrap();

        env::set_account_id(BOB);
        for i in 0..200u64 {
            map.insert(format!("game/0500/bob/{i}"), i).unwrap();
        }

        env::set_account_id(ALICE);
        let hits: Vec<String> = map.prefix(b"game/0000/").unwrap().map(|(k, _)| k).collect();
        assert_eq!(hits, vec!["game/0000/alice/1".to_owned()]);
        assert_eq!(map.len().unwrap(), 201);
    }

    #[test]
    #[serial]
    fn range_and_page_read_in_order() {
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = map();
        for k in ["a", "b", "c", "d", "e"] {
            map.insert(k.to_owned(), 1).unwrap();
        }

        let in_range: Vec<String> = map
            .range("b".to_owned().."d".to_owned())
            .unwrap()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(in_range, vec!["b".to_owned(), "c".to_owned()]);

        let page: Vec<String> = map
            .page(1, 2)
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(page, vec!["b".to_owned(), "c".to_owned()]);
    }

    #[test]
    #[serial]
    fn removing_an_entry_takes_it_out_of_the_ordered_reads() {
        // The index is maintained on remove, not only on insert — a stale index
        // would keep serving a key whose entry is gone.
        env::reset_for_testing();
        env::set_account_id(ALICE);

        let mut map = map();
        for k in ["a", "b", "c"] {
            map.insert(k.to_owned(), 1).unwrap();
        }
        assert_eq!(map.remove(&"b".to_owned()).unwrap(), Some(1));

        let keys: Vec<String> = map.keys().unwrap().collect();
        assert_eq!(keys, vec!["a".to_owned(), "c".to_owned()]);
        assert!(map.prefix(b"b").unwrap().next().is_none());
    }
}
