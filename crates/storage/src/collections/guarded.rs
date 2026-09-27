//! One write policy over any keyed collection.
//!
//! A collection answers two questions: how are its entries read, and who may
//! change them. `Guarded<C, P>` keeps them apart. The collection `C` decides
//! the first, and the policy `P` decides the second:
//!
//! ```ignore
//! posts:    Authored<IndexedMap<String, Post>>,     // only the author edits or deletes
//! comments: Authored<SortedMap<String, Comment>>,   // same, read as key ranges
//! log:      Frozen<IndexedMap<[u8; 32], Event>>,    // write once, keyed by content hash
//! ```
//!
//! [`Authored<C>`] is `Guarded<C, Owner>` and [`Frozen<C>`] is
//! `Guarded<C, Immutable>`. `C` is any [`GuardedEntries`] collection:
//! [`UnorderedMap`], [`SortedMap`] or [`IndexedMap`].
//!
//! # One stamp per entry, so one policy per collection
//!
//! A policy is a stamp on each entry (`StorageType::User { owner }` or
//! `StorageType::Frozen`), and every node checks it when it applies a
//! peer's write. The node refuses an edit or removal by anyone but the owner,
//! and any edit or removal of frozen data. That check is what makes a policy
//! hold against a peer running patched code; the checks in the methods here
//! only make a local mistake fail early. An entry carries exactly one stamp,
//! which is why policies are a parameter and not wrappers that nest.
//!
//! # Reads
//!
//! `Guarded<C, P>` dereferences to `&C`, so every read of the inner collection
//! (`range`, `prefix`, `page`, `query`, `entries`, `len`) works unchanged. It
//! never hands out `&mut C`: the only writes are the ones below, and each goes
//! through the policy.
//!
//! # Layout
//!
//! A guarded collection is its inner collection plus its own element, the
//! layout `AuthoredMap` and `FrozenStorage` have always had. The inner id is
//! derived from a per-collection prefix, so:
//!
//! * [`AuthoredMap`](super::AuthoredMap) is `Authored<UnorderedMap<K, V>>` and
//!   [`AuthoredSortedMap`](super::AuthoredSortedMap) is
//!   `Authored<SortedMap<K, V>>`, byte for byte what they were;
//! * `Authored<IndexedMap<K, V>>` stores exactly an `AuthoredMap`'s bytes, just
//!   as `IndexedMap` stores an `UnorderedMap`'s, so switching between them
//!   needs no migration;
//! * `Frozen<UnorderedMap<[u8; 32], T>>` stores exactly a
//!   [`FrozenStorage<T>`](super::FrozenStorage)'s bytes.

use core::fmt;
use core::marker::PhantomData;
use core::ops::Deref;
use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;
use sha2::{Digest, Sha256};

use super::crdt_meta::{CrdtMeta, CrdtType, MergeError, MergeStrategy, Mergeable, StorageStrategy};
use super::rekey::RekeyTarget;
use super::{authored_common, compute_id, Indexed, IndexedMap, SortedMap, StorageKey};
use super::{StoreError, UnorderedMap, ValueRef};
use crate::address::Id;
use crate::entities::{ChildInfo, Data, Element, StorageType};
use crate::index::Index;
use crate::interface::StorageError;
use crate::store::StorageAdaptor;

mod sealed {
    pub trait Sealed {}
}

/// Who may write a [`Guarded`] collection's entries.
///
/// Sealed: a policy is only as strong as the check every node runs when it
/// applies a peer's write, and those checks live in the storage layer.
pub trait Policy: sealed::Sealed + 'static {
    /// The container's `CrdtType`, which routes its merge.
    const CRDT_TYPE: CrdtType;

    /// The prefix the inner collection's id is derived from.
    fn inner_prefix<C: GuardedEntries>() -> &'static str;
}

/// Only the account that inserted an entry may update or remove it.
#[derive(Clone, Copy, Debug)]
pub struct Owner;

/// Entries are written once, keyed by the SHA-256 of their bytes, and never
/// updated or removed.
#[derive(Clone, Copy, Debug)]
pub struct Immutable;

impl sealed::Sealed for Owner {}
impl sealed::Sealed for Immutable {}

impl Policy for Owner {
    const CRDT_TYPE: CrdtType = CrdtType::UserStorage;

    fn inner_prefix<C: GuardedEntries>() -> &'static str {
        C::AUTHORED_PREFIX
    }
}

impl Policy for Immutable {
    const CRDT_TYPE: CrdtType = CrdtType::FrozenStorage;

    fn inner_prefix<C: GuardedEntries>() -> &'static str {
        "__frozen_storage_"
    }
}

/// A collection whose entries are owned by the account that inserted them.
pub type Authored<C> = Guarded<C, Owner>;

/// A content-addressed, write-once collection.
pub type Frozen<C> = Guarded<C, Immutable>;

/// A keyed collection a [`Guarded`] policy can sit on.
///
/// Sealed. Its methods are the writes a policy needs and are hidden from the
/// docs, since the only way to call them from outside is through the policy.
pub trait GuardedEntries: Data + RekeyTarget + sealed::Sealed {
    /// The key type.
    type Key: StorageKey;
    /// The value type.
    type Value: BorshSerialize + BorshDeserialize + 'static;
    /// The storage adaptor the entries live in.
    type Storage: StorageAdaptor;

    /// The prefix an owner-guarded collection's inner id is derived from.
    const AUTHORED_PREFIX: &'static str;

    #[doc(hidden)]
    fn fresh() -> Self;
    #[doc(hidden)]
    fn fresh_with_field_name(field_name: &str) -> Self;
    #[doc(hidden)]
    fn reassign(&mut self, field_name: &str);
    #[doc(hidden)]
    fn has(&self, key: &Self::Key) -> Result<bool, StoreError>;
    #[doc(hidden)]
    fn read(&self, key: &Self::Key) -> Result<Option<Self::Value>, StoreError>;
    #[doc(hidden)]
    fn insert_stamped(
        &mut self,
        key: Self::Key,
        value: Self::Value,
        stamp: StorageType,
    ) -> Result<(), StoreError>;
    /// Mutate the value at `key` in place. The entry's element, and with it
    /// its stamp, is kept; only `updated_at` advances.
    #[doc(hidden)]
    fn modify<R>(
        &mut self,
        key: &Self::Key,
        f: impl FnOnce(&mut Self::Value) -> R,
    ) -> Result<Option<R>, StoreError>;
    #[doc(hidden)]
    fn take(&mut self, key: &Self::Key) -> Result<Option<Self::Value>, StoreError>;
}

impl<K, V, S: StorageAdaptor> sealed::Sealed for UnorderedMap<K, V, S> {}
impl<K, V, S: StorageAdaptor> sealed::Sealed for SortedMap<K, V, S> {}
impl<K, V, S: StorageAdaptor> sealed::Sealed for IndexedMap<K, V, S> {}

impl<K, V, S> GuardedEntries for UnorderedMap<K, V, S>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    type Key = K;
    type Value = V;
    type Storage = S;
    const AUTHORED_PREFIX: &'static str = "__authored_map_";

    fn fresh() -> Self {
        Self::new()
    }
    fn fresh_with_field_name(field_name: &str) -> Self {
        Self::new_with_field_name(field_name)
    }
    fn reassign(&mut self, field_name: &str) {
        self.reassign_deterministic_id(field_name);
    }
    fn has(&self, key: &K) -> Result<bool, StoreError> {
        self.contains(key)
    }
    fn read(&self, key: &K) -> Result<Option<V>, StoreError> {
        Ok(self.get(key)?.map(ValueRef::into_inner))
    }
    fn insert_stamped(&mut self, key: K, value: V, stamp: StorageType) -> Result<(), StoreError> {
        let _previous = self.insert_with_storage_type(key, value, stamp, None)?;
        Ok(())
    }
    fn modify<R>(&mut self, key: &K, f: impl FnOnce(&mut V) -> R) -> Result<Option<R>, StoreError> {
        Ok(self.get_mut(key)?.map(|mut guard| f(&mut guard)))
    }
    fn take(&mut self, key: &K) -> Result<Option<V>, StoreError> {
        self.remove(key)
    }
}

impl<K, V, S> GuardedEntries for SortedMap<K, V, S>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    type Key = K;
    type Value = V;
    type Storage = S;
    const AUTHORED_PREFIX: &'static str = "__authored_sorted_map_";

    fn fresh() -> Self {
        Self::new()
    }
    fn fresh_with_field_name(field_name: &str) -> Self {
        Self::new_with_field_name(field_name)
    }
    fn reassign(&mut self, field_name: &str) {
        self.reassign_deterministic_id(field_name);
    }
    fn has(&self, key: &K) -> Result<bool, StoreError> {
        self.contains(key)
    }
    fn read(&self, key: &K) -> Result<Option<V>, StoreError> {
        Ok(self.get(key)?.map(ValueRef::into_inner))
    }
    fn insert_stamped(&mut self, key: K, value: V, stamp: StorageType) -> Result<(), StoreError> {
        let _previous = self.insert_with_storage_type(key, value, stamp, None)?;
        Ok(())
    }
    // The key set is untouched, so the ordered index needs no maintenance.
    fn modify<R>(&mut self, key: &K, f: impl FnOnce(&mut V) -> R) -> Result<Option<R>, StoreError> {
        Ok(self.get_mut(key)?.map(|mut guard| f(&mut guard)))
    }
    fn take(&mut self, key: &K) -> Result<Option<V>, StoreError> {
        self.remove(key)
    }
}

/// Stores exactly an `AuthoredMap`'s bytes, hence the same prefix.
impl<K, V, S> GuardedEntries for IndexedMap<K, V, S>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + Indexed + 'static,
    S: StorageAdaptor,
{
    type Key = K;
    type Value = V;
    type Storage = S;
    const AUTHORED_PREFIX: &'static str = "__authored_map_";

    fn fresh() -> Self {
        Self::new()
    }
    fn fresh_with_field_name(field_name: &str) -> Self {
        Self::new_with_field_name(field_name)
    }
    fn reassign(&mut self, field_name: &str) {
        self.reassign_deterministic_id(field_name);
    }
    fn has(&self, key: &K) -> Result<bool, StoreError> {
        self.contains(key)
    }
    fn read(&self, key: &K) -> Result<Option<V>, StoreError> {
        Ok(self.get(key)?.map(ValueRef::into_inner))
    }
    fn insert_stamped(&mut self, key: K, value: V, stamp: StorageType) -> Result<(), StoreError> {
        let _previous = self.insert_with_storage_type(key, value, stamp)?;
        Ok(())
    }
    // `update` keeps the indexes in step with whatever `f` changed.
    fn modify<R>(&mut self, key: &K, f: impl FnOnce(&mut V) -> R) -> Result<Option<R>, StoreError> {
        self.update(key, f)
    }
    fn take(&mut self, key: &K) -> Result<Option<V>, StoreError> {
        self.remove(key)
    }
}

/// A keyed collection `C` whose writes go through the policy `P`.
///
/// See the [module documentation](self).
#[derive(BorshSerialize, BorshDeserialize)]
pub struct Guarded<C, P> {
    #[borsh(bound(serialize = "C: BorshSerialize", deserialize = "C: BorshDeserialize"))]
    inner: C,
    storage: Element,
    #[borsh(skip)]
    policy: PhantomData<P>,
}

impl<C: GuardedEntries, P: Policy> Guarded<C, P> {
    /// Creates a new, empty collection with a random ID.
    ///
    /// Right for top-level `#[app::state]` fields (the macro reassigns a
    /// deterministic id after `init()`) and for collections nested in values.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: C::fresh(),
            storage: Element::new(None),
            policy: PhantomData,
        }
    }

    /// Creates a new, empty collection with a deterministic ID derived from
    /// `field_name`.
    #[must_use]
    pub fn new_with_field_name(field_name: &str) -> Self {
        let mut storage = Element::new_with_field_name(None, Some(field_name.to_owned()));
        storage.metadata.crdt_type = Some(P::CRDT_TYPE);
        Self {
            inner: C::fresh_with_field_name(&Self::inner_name(field_name)),
            storage,
            policy: PhantomData,
        }
    }

    /// Reassigns the collection's ID deterministically from `field_name`,
    /// migrating its entries. Called by the `#[app::state]` macro after
    /// `init()`, so top-level collections have stable IDs across nodes.
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        let new_id = super::compute_collection_id(None, field_name);
        self.storage.reassign_id_and_field_name(new_id, field_name);
        self.storage.metadata.crdt_type = Some(P::CRDT_TYPE);
        self.inner.reassign(&Self::inner_name(field_name));
    }

    /// Returns the value at `key`, if any.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn get(&self, key: &C::Key) -> Result<Option<C::Value>, StoreError> {
        self.inner.read(key)
    }

    /// Returns whether `key` is present.
    ///
    /// # Errors
    /// Returns any underlying storage error.
    pub fn contains(&self, key: &C::Key) -> Result<bool, StoreError> {
        self.inner.has(key)
    }

    fn inner_name(field_name: &str) -> String {
        format!("{}{field_name}", P::inner_prefix::<C>())
    }

    pub(crate) fn entry_id(&self, key: &C::Key) -> Id {
        compute_id(self.inner.element().id(), key.as_ref())
    }

    fn metadata_of(&self, key: &C::Key) -> Result<Option<crate::entities::Metadata>, StoreError> {
        <Index<C::Storage>>::get_metadata(self.entry_id(key)).map_err(StoreError::StorageError)
    }
}

fn not_allowed(message: &str) -> StoreError {
    StoreError::StorageError(StorageError::ActionNotAllowed(message.to_owned()))
}

impl<C: GuardedEntries> Guarded<C, Owner> {
    /// Inserts a new entry, stamping the calling account as its owner.
    ///
    /// Fails if `key` is already present: an occupied key belongs to its owner,
    /// and ownership is not transferable. Use `remove` then `insert` from the
    /// new owner.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if `key` exists, or any storage error.
    pub fn insert(&mut self, key: C::Key, value: C::Value) -> Result<(), StoreError> {
        if self.inner.has(&key)? {
            return Err(not_allowed("Authored::insert: key already exists"));
        }
        self.inner
            .insert_stamped(key, value, authored_common::make_owner_stamp())
    }

    /// Replaces the value at `key`. Only the entry's owner may call this.
    ///
    /// # Errors
    /// Returns `NotFound` if `key` is absent, `ActionNotAllowed` if the caller
    /// is not the owner, or any storage error.
    pub fn update(&mut self, key: &C::Key, value: C::Value) -> Result<(), StoreError> {
        self.modify(key, |stored| *stored = value)
    }

    /// Mutates the value at `key` in place with `f`. Only the entry's owner
    /// may call this. On an `IndexedMap` the indexes follow the change.
    ///
    /// # Errors
    /// Returns `NotFound` if `key` is absent, `ActionNotAllowed` if the caller
    /// is not the owner, or any storage error.
    pub fn modify<R>(
        &mut self,
        key: &C::Key,
        f: impl FnOnce(&mut C::Value) -> R,
    ) -> Result<R, StoreError> {
        let entry = self.entry_id(key);
        let owner = self
            .owner_of(key)?
            .ok_or(StoreError::StorageError(StorageError::NotFound(entry)))?;
        if !authored_common::writer_matches_owner(&owner) {
            return Err(not_allowed("Authored::update: not entry owner"));
        }
        self.inner
            .modify(key, f)?
            .ok_or(StoreError::StorageError(StorageError::NotFound(entry)))
    }

    /// Removes `key`. Only the entry's owner may call this.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if the caller is not the owner, or any
    /// storage error. Returns `Ok(None)` if `key` is absent.
    pub fn remove(&mut self, key: &C::Key) -> Result<Option<C::Value>, StoreError> {
        let Some(owner) = self.owner_of(key)? else {
            return Ok(None);
        };
        if !authored_common::writer_matches_owner(&owner) {
            return Err(not_allowed("Authored::remove: not entry owner"));
        }
        self.inner.take(key)
    }

    /// The account that owns `key`, if it is present.
    ///
    /// # Errors
    /// Returns any storage error.
    pub fn owner_of(&self, key: &C::Key) -> Result<Option<AccountId>, StoreError> {
        Ok(self
            .metadata_of(key)?
            .and_then(|metadata| match metadata.storage_type {
                StorageType::User { owner, .. } => Some(owner),
                _ => None,
            }))
    }

    /// Whether the calling account owns `key`. False for an absent key.
    ///
    /// # Errors
    /// Returns any storage error.
    pub fn owned_by_me(&self, key: &C::Key) -> Result<bool, StoreError> {
        Ok(self
            .owner_of(key)?
            .as_ref()
            .is_some_and(authored_common::writer_matches_owner))
    }

    /// The entry's stamped `schema_version`, or `None` if `key` is absent or
    /// was never stamped. Used to skip entries `migrate_my_entries()` already
    /// converted.
    ///
    /// # Errors
    /// Returns any storage error.
    pub fn entry_schema_version(&self, key: &C::Key) -> Result<Option<u32>, StoreError> {
        Ok(self
            .metadata_of(key)?
            .and_then(|metadata| metadata.schema_version))
    }
}

impl<C> Guarded<C, Immutable>
where
    C: GuardedEntries<Key = [u8; 32]>,
{
    /// Stores `value` under the SHA-256 of its bytes and returns that hash.
    /// Idempotent: storing equal content again returns the same hash and
    /// writes nothing.
    ///
    /// Every node recomputes the hash when it applies the entry and refuses
    /// one whose key does not match, and refuses any later update or removal.
    ///
    /// # Errors
    /// Returns any serialization or storage error.
    pub fn insert(&mut self, value: C::Value) -> Result<[u8; 32], StoreError> {
        let bytes = borsh::to_vec(&value)
            .map_err(|e| StoreError::StorageError(StorageError::SerializationError(e)))?;
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        if !self.inner.has(&hash)? {
            self.inner
                .insert_stamped(hash, value, StorageType::Frozen)?;
        }
        Ok(hash)
    }
}

impl<C: GuardedEntries, P: Policy> Default for Guarded<C, P> {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads go straight to the inner collection. There is deliberately no
/// `DerefMut`: every write goes through the policy.
impl<C, P> Deref for Guarded<C, P> {
    type Target = C;

    fn deref(&self) -> &C {
        &self.inner
    }
}

impl<C: fmt::Debug, P> fmt::Debug for Guarded<C, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl<C, P> Data for Guarded<C, P>
where
    C: GuardedEntries,
    P: 'static,
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

impl<C: GuardedEntries, P: 'static> RekeyTarget for Guarded<C, P> {
    fn rekey_relative_to(&mut self, parent_id: Id) {
        self.inner.rekey_relative_to(parent_id);
    }
}

/// Deliberately a no-op: a guarded collection never merges structurally.
///
/// Each entry arrives as its own signed action and is checked against its
/// stamp when a node applies it, and the container's `CrdtType` routes to the
/// byte-level path in `merge.rs`, so this is not reached in the normal flow.
/// Delegating to the inner collection's merge would be unsafe: a key present
/// only in `other` would be inserted with a `Public` stamp, silently stripping
/// its owner or its immutability.
#[diagnostic::do_not_recommend]
impl<C: GuardedEntries, P: 'static> Mergeable for Guarded<C, P> {
    fn merge(&mut self, _other: &Self) -> Result<(), MergeError> {
        Ok(())
    }
}

impl<C, P: Policy> CrdtMeta for Guarded<C, P> {
    fn crdt_type() -> CrdtType {
        P::CRDT_TYPE
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
impl<C, P> MergeStrategy for Guarded<C, P> {
    const DISPATCHED: bool = false;
}

#[cfg(test)]
mod tests;
