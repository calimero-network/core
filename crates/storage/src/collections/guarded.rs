//! One write policy over any keyed collection.
//!
//! A collection answers two questions: how are its entries read, and who may
//! change them. `Guarded<C, P>` keeps them apart. The collection `C` decides
//! the first, and the policy `P` decides the second:
//!
//! ```ignore
//! posts:    Authored<IndexedMap<String, Post>>,     // only the author edits or deletes
//! comments: Authored<SortedMap<String, Comment>>,   // same, read as key ranges
//! log:      ContentAddressed<IndexedMap<[u8; 32], Event>>,    // write once, keyed by content hash
//! ```
//!
//! [`Authored<C>`] is `Guarded<C, Owner>` and [`ContentAddressed<C>`] is
//! `Guarded<C, ContentHash>`. `C` is any [`GuardedEntries`] collection:
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
//! # Keys are per owner
//!
//! Under an owning policy (`Authored`, `WriteOnce`, `Moderated`,
//! `ModeratedOnce`) each account has its own namespace: an entry's id is
//! derived from its key AND its owner, so two accounts writing one key hold two
//! independent entries, on every node, in whatever order the writes arrive.
//! Every key-only method (`insert`, `get`, `contains`, `update`, `modify`,
//! `remove`, `owner_of`, `owned_by_me`, `entry_schema_version`) acts on the
//! CALLER's entry. Another account's entry is read by naming the owner:
//! [`get_by`](Guarded::get_by), [`contains_by`](Guarded::contains_by),
//! [`entry_schema_version_by`](Guarded::entry_schema_version_by), and a
//! moderator removes one with `remove_by`. A name unique across the whole
//! collection needs content addressing (`ContentAddressed`) or moderation, not
//! an owning policy.
//!
//! # Reads
//!
//! `Guarded<C, P>` dereferences to `&C`, so every read of the inner collection
//! (`range`, `prefix`, `page`, `query`, `entries`, `len`) works unchanged. Under
//! an owning policy those span every owner: one key appears once per owner that
//! holds it, ordered by key and then by entry id, and `len` counts them all.
//! [`entries_with_owners`](Guarded::entries_with_owners),
//! [`entries_by`](Guarded::entries_by), [`my_entries`](Guarded::my_entries) and
//! [`entries_at`](Guarded::entries_at) say whose each entry is. It never hands
//! out `&mut C`: the only writes are the ones below, and each goes through the
//! policy.
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
//! * `ContentAddressed<UnorderedMap<[u8; 32], T>>` stores exactly a
//!   [`FrozenStorage<T>`](super::FrozenStorage)'s bytes.

use core::fmt;
use core::marker::PhantomData;
use core::ops::Deref;
use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;
use sha2::{Digest, Sha256};

use super::crdt_meta::{CrdtMeta, CrdtType, MergeError, MergeStrategy, Mergeable, StorageStrategy};
use super::rekey::RekeyTarget;
use super::{
    authored_common, compute_id, owned_entry_id, Indexed, IndexedMap, LwwRegister, SortedMap,
};
use super::{StorageKey, WriterSetCell};
use super::{StoreError, UnorderedMap, ValueRef};
use crate::address::Id;
use crate::domain::Domain;
use crate::entities::{ChildInfo, Data, Element, EntryRules, StorageType};
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
///
/// A policy is stored with the collection. The stateless ones are unit structs
/// and store nothing, so they add no bytes to the layout; [`Moderation`] stores
/// its moderators' writer set.
pub trait Policy: sealed::Sealed + BorshSerialize + BorshDeserialize + 'static {
    /// The container's `CrdtType`, which routes its merge.
    const CRDT_TYPE: CrdtType;

    /// The prefix the inner collection's id is derived from.
    fn inner_prefix<C: GuardedEntries>() -> &'static str;

    /// A policy for a new collection, created by the calling account.
    fn fresh() -> Self;

    /// Give the policy's own state, if any, deterministic ids under
    /// `field_name`, as the `#[app::state]` macro does after `init()`.
    fn reassign(&mut self, _field_name: &str) {}

    /// Which entry stamps belong to a collection under this policy. Every
    /// other entry reads as absent (see [`crate::domain`]).
    fn domain(&self) -> Domain;
}

/// A policy under which each entry is owned by the account that wrote it, and
/// carries the [`EntryRules`] the policy decides.
pub trait Owning: Policy {
    /// The rules every entry is created with.
    fn rules(&self) -> EntryRules;
}

/// An owning policy whose entries their owner may still change.
pub trait OwnerEdits: Owning {}

/// Only the account that inserted an entry may update or remove it.
#[derive(Clone, Copy, Debug, BorshSerialize, BorshDeserialize)]
pub struct Owner;

/// Each entry is owned by the account that inserted it and written once:
/// every node refuses an update or delete of it, by anyone.
#[derive(Clone, Copy, Debug, BorshSerialize, BorshDeserialize)]
pub struct OwnerOnce;

/// Entries are written once, keyed by the SHA-256 of their bytes, and never
/// updated or removed.
#[derive(Clone, Copy, Debug, BorshSerialize, BorshDeserialize)]
pub struct ContentHash;

/// Whether a moderated collection's entries may be edited by their owner.
pub trait Edits: sealed::Sealed + 'static {
    /// True if an entry is written once.
    const IMMUTABLE: bool;
}

/// The owner may edit and remove their entry.
#[derive(Clone, Copy, Debug)]
pub struct Editable;

/// Nobody may edit an entry; only a moderator may remove it.
#[derive(Clone, Copy, Debug)]
pub struct Once;

impl sealed::Sealed for Editable {}
impl sealed::Sealed for Once {}
impl Edits for Editable {
    const IMMUTABLE: bool = false;
}
impl Edits for Once {
    const IMMUTABLE: bool = true;
}

/// Each entry is owned by the account that inserted it, and a set of
/// moderators may remove any entry. The moderators are a writer set, verified
/// and rotated exactly as a [`WriterSetCell`]'s: every node checks a removal
/// against the moderators as of that removal.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct Moderation<E> {
    moderators: WriterSetCell<LwwRegister<bool>>,
    #[borsh(skip)]
    edits: PhantomData<E>,
}

impl<E> fmt::Debug for Moderation<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Moderation")
            .field("moderators", &self.moderators.writers())
            .finish()
    }
}

impl sealed::Sealed for Owner {}
impl sealed::Sealed for OwnerOnce {}
impl sealed::Sealed for ContentHash {}
impl<E: Edits> sealed::Sealed for Moderation<E> {}

impl Policy for Owner {
    const CRDT_TYPE: CrdtType = CrdtType::UserStorage;

    fn inner_prefix<C: GuardedEntries>() -> &'static str {
        C::AUTHORED_PREFIX
    }
    fn fresh() -> Self {
        Self
    }
    fn domain(&self) -> Domain {
        Domain::Owned(EntryRules::OWNED)
    }
}

impl Owning for Owner {
    fn rules(&self) -> EntryRules {
        EntryRules::OWNED
    }
}
impl OwnerEdits for Owner {}

impl Policy for OwnerOnce {
    const CRDT_TYPE: CrdtType = CrdtType::UserStorage;

    fn inner_prefix<C: GuardedEntries>() -> &'static str {
        "__write_once_"
    }
    fn fresh() -> Self {
        Self
    }
    fn domain(&self) -> Domain {
        Domain::Owned(self.rules())
    }
}

impl Owning for OwnerOnce {
    fn rules(&self) -> EntryRules {
        EntryRules {
            immutable: true,
            moderators: None,
        }
    }
}

impl Policy for ContentHash {
    const CRDT_TYPE: CrdtType = CrdtType::FrozenStorage;

    fn inner_prefix<C: GuardedEntries>() -> &'static str {
        "__frozen_storage_"
    }
    fn fresh() -> Self {
        Self
    }
    fn domain(&self) -> Domain {
        Domain::ContentAddressed
    }
}

impl<E: Edits> Policy for Moderation<E> {
    const CRDT_TYPE: CrdtType = CrdtType::UserStorage;

    fn inner_prefix<C: GuardedEntries>() -> &'static str {
        if E::IMMUTABLE {
            "__moderated_once_"
        } else {
            "__moderated_"
        }
    }
    /// The calling account is the first moderator.
    fn fresh() -> Self {
        Self::with_moderators([authored_common::current_writer()].into_iter().collect())
    }
    fn reassign(&mut self, field_name: &str) {
        self.moderators
            .reassign_deterministic_id(&format!("__moderators_{field_name}"));
    }
    fn domain(&self) -> Domain {
        Domain::Owned(self.rules())
    }
}

impl<E: Edits> Owning for Moderation<E> {
    fn rules(&self) -> EntryRules {
        EntryRules {
            immutable: E::IMMUTABLE,
            moderators: Some(self.moderators.anchor()),
        }
    }
}
impl OwnerEdits for Moderation<Editable> {}

impl<E: Edits> Moderation<E> {
    fn with_moderators(moderators: BTreeSet<AccountId>) -> Self {
        Self {
            moderators: WriterSetCell::new(moderators, false),
            edits: PhantomData,
        }
    }
}

/// A collection whose entries are owned by the account that inserted them.
pub type Authored<C> = Guarded<C, Owner>;

/// A collection whose entries are owned by the account that inserted them and
/// written once: signed chat messages, votes cast, receipts. Nobody, the
/// author included, can change or remove one.
pub type WriteOnce<C> = Guarded<C, OwnerOnce>;

/// A collection whose entries their author owns and edits, and any moderator
/// can remove.
pub type Moderated<C> = Guarded<C, Moderation<Editable>>;

/// A collection whose entries are written once by their author, and any
/// moderator can remove: a chat nobody can rewrite, but whose spam can go.
pub type ModeratedOnce<C> = Guarded<C, Moderation<Once>>;

/// A content-addressed, write-once collection: each entry is keyed by the
/// SHA-256 of its bytes.
pub type ContentAddressed<C> = Guarded<C, ContentHash>;

/// What a guarded collection is keyed by and stored in: all that reading an
/// entry's stamp needs. Split from [`GuardedEntries`] so the stamp readers
/// (`owner_of`, `owned_by_me`, `entry_schema_version`) ask nothing of the
/// value type, as `AuthoredMap`'s always did: generic code over
/// `AuthoredMap<K, V>` need not bound `V: 'static` to read an owner.
pub trait GuardedKeys: Data + sealed::Sealed {
    /// The key type.
    type Key: StorageKey;
    /// The storage adaptor the entries live in.
    type Storage: StorageAdaptor;
}

/// A keyed collection a [`Guarded`] policy can sit on.
///
/// Sealed. Its methods are the writes a policy needs and are hidden from the
/// docs, since the only way to call them from outside is through the policy.
pub trait GuardedEntries: GuardedKeys + RekeyTarget {
    /// The value type.
    type Value: BorshSerialize + BorshDeserialize + 'static;

    /// The prefix an owner-guarded collection's inner id is derived from.
    const AUTHORED_PREFIX: &'static str;

    #[doc(hidden)]
    fn fresh() -> Self;
    #[doc(hidden)]
    fn fresh_with_field_name(field_name: &str) -> Self;
    #[doc(hidden)]
    fn reassign(&mut self, field_name: &str);
    /// Let reads check each owned entry's key against its slot.
    #[doc(hidden)]
    fn bind_slot_keys(&mut self);
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
    /// `owner`'s value at `key`.
    #[doc(hidden)]
    fn read_by(
        &self,
        owner: &AccountId,
        key: &Self::Key,
    ) -> Result<Option<Self::Value>, StoreError>;
    /// Remove `owner`'s entry at `key`.
    #[doc(hidden)]
    fn take_by(
        &mut self,
        owner: &AccountId,
        key: &Self::Key,
    ) -> Result<Option<Self::Value>, StoreError>;
    /// Every entry with its owner: only `owner`'s when given.
    #[doc(hidden)]
    fn owned_entries(&self, owner: Option<&AccountId>) -> Result<OwnedEntries<Self>, StoreError>;
    /// Every owner's value at `key`, ascending by entry id.
    #[doc(hidden)]
    fn entries_at(&self, key: &Self::Key) -> Result<Vec<(AccountId, Self::Value)>, StoreError>;
}

/// Every entry of a guarded collection with its owner.
pub type OwnedEntries<C> = Vec<(
    AccountId,
    <C as GuardedKeys>::Key,
    <C as GuardedEntries>::Value,
)>;

/// Entries of a guarded collection, as `(key, value)`.
pub type KeyedEntries<C> = Vec<(<C as GuardedKeys>::Key, <C as GuardedEntries>::Value)>;

impl<K, V, S: StorageAdaptor> sealed::Sealed for UnorderedMap<K, V, S> {}
impl<K, V, S: StorageAdaptor> sealed::Sealed for SortedMap<K, V, S> {}
impl<K, V, S: StorageAdaptor> sealed::Sealed for IndexedMap<K, V, S> {}

impl<K, V, S> GuardedKeys for UnorderedMap<K, V, S>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    type Key = K;
    type Storage = S;
}

impl<K, V, S> GuardedKeys for SortedMap<K, V, S>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    type Key = K;
    type Storage = S;
}

impl<K, V, S> GuardedKeys for IndexedMap<K, V, S>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    type Key = K;
    type Storage = S;
}

impl<K, V, S> GuardedEntries for UnorderedMap<K, V, S>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    type Value = V;
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
    fn bind_slot_keys(&mut self) {
        Self::bind_slot_keys(self);
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
    fn read_by(&self, owner: &AccountId, key: &K) -> Result<Option<V>, StoreError> {
        self.get_by_owner(owner, key)
    }
    fn take_by(&mut self, owner: &AccountId, key: &K) -> Result<Option<V>, StoreError> {
        self.remove_by_owner(owner, key)
    }
    fn owned_entries(&self, owner: Option<&AccountId>) -> Result<OwnedEntries<Self>, StoreError> {
        Self::owned_entries(self, owner)
    }
    fn entries_at(&self, key: &K) -> Result<Vec<(AccountId, V)>, StoreError> {
        Self::entries_at(self, key)
    }
}

impl<K, V, S> GuardedEntries for SortedMap<K, V, S>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    type Value = V;
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
    fn bind_slot_keys(&mut self) {
        Self::bind_slot_keys(self);
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
    fn read_by(&self, owner: &AccountId, key: &K) -> Result<Option<V>, StoreError> {
        self.get_by_owner(owner, key)
    }
    fn take_by(&mut self, owner: &AccountId, key: &K) -> Result<Option<V>, StoreError> {
        self.remove_by_owner(owner, key)
    }
    fn owned_entries(&self, owner: Option<&AccountId>) -> Result<OwnedEntries<Self>, StoreError> {
        Self::owned_entries(self, owner)
    }
    fn entries_at(&self, key: &K) -> Result<Vec<(AccountId, V)>, StoreError> {
        Self::entries_at(self, key)
    }
}

/// Stores exactly an `AuthoredMap`'s bytes, hence the same prefix.
impl<K, V, S> GuardedEntries for IndexedMap<K, V, S>
where
    K: StorageKey,
    V: BorshSerialize + BorshDeserialize + Indexed + 'static,
    S: StorageAdaptor,
{
    type Value = V;
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
    fn bind_slot_keys(&mut self) {
        Self::bind_slot_keys(self);
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
    fn read_by(&self, owner: &AccountId, key: &K) -> Result<Option<V>, StoreError> {
        self.get_by_owner(owner, key)
    }
    fn take_by(&mut self, owner: &AccountId, key: &K) -> Result<Option<V>, StoreError> {
        self.remove_by_owner(owner, key)
    }
    fn owned_entries(&self, owner: Option<&AccountId>) -> Result<OwnedEntries<Self>, StoreError> {
        Self::owned_entries(self, owner)
    }
    fn entries_at(&self, key: &K) -> Result<Vec<(AccountId, V)>, StoreError> {
        Self::entries_at(self, key)
    }
}

/// A keyed collection `C` whose writes go through the policy `P`.
///
/// See the [module documentation](self).
#[derive(BorshSerialize)]
pub struct Guarded<C, P> {
    #[borsh(bound(serialize = "C: BorshSerialize"))]
    inner: C,
    storage: Element,
    #[borsh(bound(serialize = "P: BorshSerialize"))]
    policy: P,
}

/// The stored layout is the derive's; the policy's domain is set on the inner
/// collection as it comes back, whatever domain it was loaded in.
impl<C: GuardedEntries, P: Policy> BorshDeserialize for Guarded<C, P> {
    fn deserialize_reader<R: borsh::io::Read>(reader: &mut R) -> borsh::io::Result<Self> {
        let inner = C::deserialize_reader(reader)?;
        let storage = Element::deserialize_reader(reader)?;
        let policy = P::deserialize_reader(reader)?;
        Ok(Self::from_parts(inner, storage, policy))
    }
}

impl<C: GuardedEntries, P: Policy> Guarded<C, P> {
    /// Creates a new, empty collection with a random ID.
    ///
    /// Right for top-level `#[app::state]` fields (the macro reassigns a
    /// deterministic id after `init()`) and for collections nested in values.
    #[must_use]
    pub fn new() -> Self {
        Self::from_parts(C::fresh(), Element::new(None), P::fresh())
    }

    fn from_parts(mut inner: C, storage: Element, policy: P) -> Self {
        inner.element_mut().domain = policy.domain();
        inner.bind_slot_keys();
        Self {
            inner,
            storage,
            policy,
        }
    }

    /// Creates a new, empty collection with a deterministic ID derived from
    /// `field_name`.
    #[must_use]
    pub fn new_with_field_name(field_name: &str) -> Self {
        let mut storage = Element::new_with_field_name(None, Some(field_name.to_owned()));
        storage.metadata.crdt_type = Some(P::CRDT_TYPE);
        let mut policy = P::fresh();
        policy.reassign(field_name);
        Self::from_parts(
            C::fresh_with_field_name(&Self::inner_name(field_name)),
            storage,
            policy,
        )
    }

    /// Reassigns the collection's ID deterministically from `field_name`,
    /// migrating its entries. Called by the `#[app::state]` macro after
    /// `init()`, so top-level collections have stable IDs across nodes.
    pub fn reassign_deterministic_id(&mut self, field_name: &str) {
        let new_id = super::compute_collection_id(None, field_name);
        self.storage.reassign_id_and_field_name(new_id, field_name);
        self.storage.metadata.crdt_type = Some(P::CRDT_TYPE);
        self.inner.reassign(&Self::inner_name(field_name));
        // A policy with state (the moderators' cell) moves too, and the domain
        // follows it: moderated entries name that cell's id.
        self.policy.reassign(field_name);
        self.inner.element_mut().domain = self.policy.domain();
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
}

impl<C: GuardedKeys, P: Owning> Guarded<C, P> {
    /// The id the calling account's entry at `key` is stored under.
    pub(crate) fn entry_id(&self, key: &C::Key) -> Id {
        self.entry_id_of(&authored_common::current_writer(), key)
    }

    /// The id `owner`'s entry at `key` is stored under.
    pub(crate) fn entry_id_of(&self, owner: &AccountId, key: &C::Key) -> Id {
        owned_entry_id(compute_id(self.inner.element().id(), key.as_ref()), owner)
    }

    fn metadata_of(
        &self,
        owner: &AccountId,
        key: &C::Key,
    ) -> Result<Option<crate::entities::Metadata>, StoreError> {
        <Index<C::Storage>>::get_metadata(self.entry_id_of(owner, key))
            .map_err(StoreError::StorageError)
    }
}

#[cfg(test)]
impl<C: GuardedKeys> Guarded<C, ContentHash> {
    /// The id the entry at `key` is stored under: content-addressed entries
    /// are nobody's, so it is the key's own.
    pub(crate) fn entry_id(&self, key: &C::Key) -> Id {
        compute_id(self.inner.element().id(), key.as_ref())
    }
}

fn not_allowed(message: &str) -> StoreError {
    StoreError::StorageError(StorageError::ActionNotAllowed(message.to_owned()))
}

impl<C: GuardedEntries, P: Owning> Guarded<C, P> {
    /// Inserts a new entry at `key` in the calling account's namespace,
    /// stamping it as the owner.
    ///
    /// Fails only if the caller already holds `key`: another account's entry
    /// at the same key is a different entry. Use `update` to change yours.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if the caller holds `key`, or any storage
    /// error.
    pub fn insert(&mut self, key: C::Key, value: C::Value) -> Result<(), StoreError> {
        if self.inner.has(&key)? {
            return Err(not_allowed("Authored::insert: key already exists"));
        }
        self.inner.insert_stamped(
            key,
            value,
            authored_common::make_owner_stamp_with(self.policy.rules()),
        )
    }

    /// `owner`'s value at `key`, if they hold it.
    ///
    /// Authorization-shaped logic ("may this account do X to the thing at
    /// `key`?") must read the entry of the account it is about, by name.
    ///
    /// # Errors
    /// Returns any storage error.
    pub fn get_by(&self, owner: &AccountId, key: &C::Key) -> Result<Option<C::Value>, StoreError> {
        self.inner.read_by(owner, key)
    }

    /// Whether `owner` holds `key`.
    ///
    /// # Errors
    /// Returns any storage error.
    pub fn contains_by(&self, owner: &AccountId, key: &C::Key) -> Result<bool, StoreError> {
        Ok(self.get_by(owner, key)?.is_some())
    }

    /// Every entry with the account that owns it, in storage order.
    ///
    /// # Errors
    /// Returns any storage error.
    pub fn entries_with_owners(&self) -> Result<OwnedEntries<C>, StoreError> {
        self.inner.owned_entries(None)
    }

    /// `owner`'s entries, in storage order.
    ///
    /// # Errors
    /// Returns any storage error.
    pub fn entries_by(&self, owner: &AccountId) -> Result<KeyedEntries<C>, StoreError> {
        Ok(self
            .inner
            .owned_entries(Some(owner))?
            .into_iter()
            .map(|(_, key, value)| (key, value))
            .collect())
    }

    /// The calling account's entries, in storage order.
    ///
    /// # Errors
    /// Returns any storage error.
    pub fn my_entries(&self) -> Result<KeyedEntries<C>, StoreError> {
        self.entries_by(&authored_common::current_writer())
    }

    /// Every account's value at `key`, ascending by entry id: one trie bucket
    /// read, however many entries the collection holds.
    ///
    /// # Errors
    /// Returns any storage error.
    pub fn entries_at(&self, key: &C::Key) -> Result<Vec<(AccountId, C::Value)>, StoreError> {
        self.inner.entries_at(key)
    }
}

impl<C: GuardedKeys, P: Owning> Guarded<C, P> {
    /// The calling account, if it holds `key`: keys are per owner, so no one
    /// else's entry answers this. Use [`contains_by`](Self::contains_by) to ask
    /// about another account.
    ///
    /// # Errors
    /// Returns any storage error.
    pub fn owner_of(&self, key: &C::Key) -> Result<Option<AccountId>, StoreError> {
        Ok(self
            .metadata_of(&authored_common::current_writer(), key)?
            .and_then(|metadata| match metadata.storage_type {
                StorageType::User { owner, .. } => Some(owner),
                _ => None,
            }))
    }

    /// Whether the calling account holds `key`: the same answer as
    /// `contains`, read from the entry's stamp alone.
    ///
    /// # Errors
    /// Returns any storage error.
    pub fn owned_by_me(&self, key: &C::Key) -> Result<bool, StoreError> {
        Ok(self
            .owner_of(key)?
            .as_ref()
            .is_some_and(authored_common::writer_matches_owner))
    }

    /// The calling account's entry's stamped `schema_version`, or `None` if
    /// it does not hold `key` or the entry was never stamped. Used to skip
    /// entries `migrate_my_entries()` already converted.
    ///
    /// # Errors
    /// Returns any storage error.
    pub fn entry_schema_version(&self, key: &C::Key) -> Result<Option<u32>, StoreError> {
        self.entry_schema_version_by(&authored_common::current_writer(), key)
    }

    /// `owner`'s entry's stamped `schema_version`, or `None` if they do not
    /// hold `key` or the entry was never stamped.
    ///
    /// # Errors
    /// Returns any storage error.
    pub fn entry_schema_version_by(
        &self,
        owner: &AccountId,
        key: &C::Key,
    ) -> Result<Option<u32>, StoreError> {
        Ok(self
            .metadata_of(owner, key)?
            .and_then(|metadata| metadata.schema_version))
    }
}

impl<C: GuardedEntries, P: OwnerEdits> Guarded<C, P> {
    /// Replaces the value of the calling account's entry at `key`.
    ///
    /// # Errors
    /// Returns `NotFound` if the caller does not hold `key`, or any storage
    /// error.
    pub fn update(&mut self, key: &C::Key, value: C::Value) -> Result<(), StoreError> {
        self.modify(key, |stored| *stored = value)
    }

    /// Mutates the value of the calling account's entry at `key` in place with
    /// `f`. On an `IndexedMap` the indexes follow the change.
    ///
    /// # Errors
    /// Returns `NotFound` if the caller does not hold `key`, or any storage
    /// error.
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
}

impl<C: GuardedEntries> Guarded<C, Owner> {
    /// Removes the calling account's entry at `key`.
    ///
    /// # Errors
    /// Returns any storage error. Returns `Ok(None)` if the caller does not
    /// hold `key`.
    pub fn remove(&mut self, key: &C::Key) -> Result<Option<C::Value>, StoreError> {
        let Some(owner) = self.owner_of(key)? else {
            return Ok(None);
        };
        if !authored_common::writer_matches_owner(&owner) {
            return Err(not_allowed("Authored::remove: not entry owner"));
        }
        self.inner.take(key)
    }
}

impl<C: GuardedEntries, E: Edits> Guarded<C, Moderation<E>> {
    /// Removes the calling account's entry at `key`, unless the collection's
    /// entries are written once and the caller is no moderator.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if the caller may not, or any storage error.
    /// Returns `Ok(None)` if the caller does not hold `key`.
    pub fn remove(&mut self, key: &C::Key) -> Result<Option<C::Value>, StoreError> {
        self.remove_by(&authored_common::current_writer(), key)
    }

    /// Removes `owner`'s entry at `key`. A moderator may remove anyone's entry;
    /// its owner may remove it unless the collection's entries are written
    /// once. Every node checks a moderator's removal against the moderators as
    /// of that removal.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if the caller may not, or any storage error.
    /// Returns `Ok(None)` if `owner` does not hold `key`.
    pub fn remove_by(
        &mut self,
        owner: &AccountId,
        key: &C::Key,
    ) -> Result<Option<C::Value>, StoreError> {
        if self.metadata_of(owner, key)?.is_none() {
            return Ok(None);
        }
        let by_owner = !E::IMMUTABLE && authored_common::writer_matches_owner(owner);
        if !by_owner && !self.is_moderator(&authored_common::current_writer()) {
            return Err(not_allowed(
                "Moderated::remove: neither a moderator nor the entry's editing owner",
            ));
        }
        self.inner.take_by(owner, key)
    }

    /// The current moderators.
    #[must_use]
    pub fn moderators(&self) -> BTreeSet<AccountId> {
        self.policy.moderators.writers()
    }

    /// Whether `account` may remove any entry.
    #[must_use]
    pub fn is_moderator(&self, account: &AccountId) -> bool {
        self.policy
            .moderators
            .capabilities()
            .get(account)
            .is_some_and(|mask| mask.contains(crate::entities::OpMask::DELETE))
    }

    /// Replace the moderators. Only a current moderator may, and every node
    /// verifies it as a writer-set rotation; a removal checks the moderators as
    /// of that removal, so revoking someone stops their later removals only.
    ///
    /// # Errors
    /// Returns `ActionNotAllowed` if the caller is not a moderator or the set is
    /// empty, or any storage error.
    pub fn set_moderators(&mut self, moderators: BTreeSet<AccountId>) -> Result<(), StoreError> {
        self.policy.moderators.rotate_writers(moderators)
    }
}

impl<C> Guarded<C, ContentHash>
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
    P: Policy,
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

impl<C: GuardedEntries, P: Policy> RekeyTarget for Guarded<C, P> {
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
impl<C: GuardedEntries, P: Policy> Mergeable for Guarded<C, P> {
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
