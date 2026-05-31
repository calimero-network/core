//! An ordered key-value map supporting range and prefix queries.
//!
//! [`SortedMap`] stores its entries exactly like [`UnorderedMap`](super::UnorderedMap)
//! — a single inner [`Collection`] of `(K, V)` pairs keyed by
//! `compute_id(parent, key)` — so it inherits the *identical* CRDT merge
//! semantics (add-wins keys, recursive value merge, per-key tombstones, and the
//! nested-CRDT deterministic re-keying from [`super::rekey`]) and on-wire byte
//! layout. Nothing extra is synced.
//!
//! What it adds is an **ordered view**. Because entry ids are
//! `SHA256(parent ‖ key)`, entity-id order ≠ key order, and the underlying
//! store cannot seek by key (see core#2559). `SortedMap` therefore materialises
//! the key order *in memory* on each ordered read: it reads the entries once and
//! sorts them by `K: Ord`. The order is a pure function of the key set, so every
//! replica derives the same order with **zero extra merge logic** — that is what
//! makes the ordering CRDT-safe.
//!
//! This deliberately does not pretend to do sub-linear disk seeks (impossible
//! while keys are hashed). Its value is ergonomic: it encapsulates and amortises
//! the `keys.sort()` dance apps used to write by hand, and exposes
//! [`range`](SortedMap::range), [`prefix`](SortedMap::prefix),
//! [`page`](SortedMap::page) and sorted [`entries`](SortedMap::entries) /
//! [`keys`](SortedMap::keys) / [`values`](SortedMap::values) directly.

use core::borrow::Borrow;
use core::fmt;
use core::ops::{Deref, DerefMut, RangeBounds};
use std::mem;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::ser::SerializeMap;
use serde::Serialize;

use super::{compute_id, Collection, CrdtType, EntryMut, StorageAdaptor};
use crate::address::Id;
use crate::collections::error::StoreError;
use crate::entities::{ChildInfo, Data, Element, StorageType};
use crate::error::StorageError;
use crate::store::MainStorage;
use std::collections::BTreeMap;

/// A map collection that keeps its entries ordered by key, enabling range and
/// prefix queries plus pagination.
///
/// See the [module documentation](self) for the storage model and the
/// CRDT-safety argument.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct SortedMap<K, V, S: StorageAdaptor = MainStorage> {
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: Collection<(K, V), S>,
}

/// Re-key a nested sorted map (one stored as another collection's value)
/// relative to its storage parent — mirrors [`UnorderedMap`](super::UnorderedMap)
/// so a nested `SortedMap` value's children converge across nodes. See
/// [`super::rekey`].
impl<K, V, S> super::rekey::RekeyTarget for SortedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq + 'static,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    fn rekey_relative_to(&mut self, parent_id: Id) {
        self.reassign_deterministic_id_under(
            parent_id,
            "__nested_sorted_map",
            CrdtType::sorted_map(std::any::type_name::<K>(), std::any::type_name::<V>()),
        );
    }
}

impl<K, V, S> SortedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Create a new sorted map with a random ID.
    ///
    /// Use this for nested collections stored as values in other maps. Merge
    /// happens by the parent map's key, so the nested collection's ID doesn't
    /// affect sync semantics.
    ///
    /// For top-level state fields, use [`new_with_field_name`](Self::new_with_field_name).
    pub fn new() -> Self {
        Self::new_internal()
    }

    /// Create a new sorted map with a deterministic ID derived from `field_name`.
    ///
    /// Use this for top-level state fields (the `#[app::state]` macro does this
    /// automatically).
    ///
    /// # Example
    /// ```ignore
    /// let items = SortedMap::<String, String>::new_with_field_name("items");
    /// ```
    pub fn new_with_field_name(field_name: &str) -> Self {
        Self::new_with_field_name_internal(None, field_name)
    }
}

impl<K, V, S> SortedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Create a new sorted map (internal).
    pub(super) fn new_internal() -> Self {
        Self {
            inner: Collection::new(None),
        }
    }

    /// Create a new sorted map with a deterministic ID (internal).
    pub(super) fn new_with_field_name_internal(
        parent_id: Option<crate::address::Id>,
        field_name: &str,
    ) -> Self {
        Self {
            inner: Collection::new_with_field_name_and_crdt_type(
                parent_id,
                field_name,
                CrdtType::sorted_map(std::any::type_name::<K>(), std::any::type_name::<V>()),
            ),
        }
    }

    /// Updates the CRDT type metadata on the map collection itself.
    pub(crate) fn set_collection_crdt_type(&mut self, crdt_type: CrdtType) {
        self.inner.element_mut().metadata.crdt_type = Some(crdt_type);
    }

    /// Reassign the map's id + collection CRDT type to a deterministic value
    /// keyed under `parent_id` (`None` = top-level / ROOT-relative). Shared
    /// implementation behind the two `reassign_deterministic_id_*` entry points.
    ///
    /// Migration is: snapshot entries → clear (drops old-id entries) → reassign
    /// the collection id → re-insert (each entry, and its own nested values via
    /// `insert`'s re-key, gets a new deterministic id under the new parent). The
    /// snapshot uses unordered iteration: re-insert order is irrelevant because
    /// each entry's new id is a pure function of its key.
    #[expect(clippy::expect_used, reason = "fatal error if migration fails")]
    fn reassign_deterministic_id_keyed(
        &mut self,
        parent_id: Option<Id>,
        field_name: &str,
        crdt_type: CrdtType,
    ) where
        K: AsRef<[u8]> + PartialEq + 'static,
        V: 'static,
    {
        let new_id = super::compute_collection_id(parent_id, field_name);
        let old_id = self.inner.id();

        // If already has the correct ID, only ensure CRDT type is correct.
        if old_id == new_id {
            self.set_collection_crdt_type(crdt_type);
            return;
        }

        // Snapshot all entries before migration (must do this before clearing).
        let entries: Vec<(K, V)> = self
            .iter_unordered()
            .expect("failed to read entries for re-key")
            .collect();

        // Clear the collection (removes old entries with old IDs).
        self.inner.clear().expect("failed to clear for re-key");

        // Reassign the collection's ID (Collection's `_with_crdt_type` is itself
        // just `_under(None, ..)`, so this single call covers both variants).
        self.inner
            .reassign_deterministic_id_under(parent_id, field_name, crdt_type);

        // Re-insert all entries (they will get new IDs based on new parent ID).
        for (key, value) in entries {
            self.insert(key, value)
                .expect("failed to re-insert entry during re-key");
        }
    }

    /// Reassigns the map's ID and collection CRDT type to deterministic values.
    pub(crate) fn reassign_deterministic_id_with_crdt_type(
        &mut self,
        field_name: &str,
        crdt_type: CrdtType,
    ) where
        K: AsRef<[u8]> + PartialEq + 'static,
        V: 'static,
    {
        self.reassign_deterministic_id_keyed(None, field_name, crdt_type);
    }

    /// Like [`reassign_deterministic_id_with_crdt_type`], but keys the new id
    /// relative to `parent_id` (for a map nested inside another entity).
    ///
    /// [`reassign_deterministic_id_with_crdt_type`]: Self::reassign_deterministic_id_with_crdt_type
    pub(crate) fn reassign_deterministic_id_under(
        &mut self,
        parent_id: Id,
        field_name: &str,
        crdt_type: CrdtType,
    ) where
        K: AsRef<[u8]> + PartialEq + 'static,
        V: 'static,
    {
        self.reassign_deterministic_id_keyed(Some(parent_id), field_name, crdt_type);
    }

    /// Reassigns the map's ID to a deterministic ID based on field name,
    /// migrating all existing entries to the new parent ID.
    ///
    /// Called by the `#[app::state]` macro after `init()` returns so every
    /// top-level collection ends up with a deterministic ID regardless of how it
    /// was created in `init()`.
    ///
    /// # Arguments
    /// * `field_name` - The name of the struct field containing this map
    pub fn reassign_deterministic_id(&mut self, field_name: &str)
    where
        K: AsRef<[u8]> + PartialEq + 'static,
        V: 'static,
    {
        self.reassign_deterministic_id_with_crdt_type(
            field_name,
            CrdtType::sorted_map(std::any::type_name::<K>(), std::any::type_name::<V>()),
        );
    }

    /// Insert a key-value pair into the map.
    ///
    /// Returns the previous value for `key` if one existed.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    pub fn insert(&mut self, key: K, value: V) -> Result<Option<V>, StoreError>
    where
        K: AsRef<[u8]> + PartialEq + 'static,
        V: 'static,
    {
        self.insert_with_storage_type(key, value, StorageType::Public, None)
    }

    /// Insert a key-value pair with the specified `StorageType` and optional
    /// custom `Id`.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    pub(crate) fn insert_with_storage_type(
        &mut self,
        key: K,
        mut value: V,
        storage_type: StorageType,
        custom_id: Option<Id>,
    ) -> Result<Option<V>, StoreError>
    where
        K: AsRef<[u8]> + PartialEq + 'static,
        V: 'static,
    {
        // Register this map type's nested-id re-key thunk, so a map stored as
        // another collection's value (map-of-map) is re-keyed when that outer
        // collection is itself stored (see `super::rekey`).
        super::rekey::register_rekey::<Self>();

        let id = custom_id.unwrap_or_else(|| compute_id(self.inner.id(), key.as_ref()));

        // Re-key any nested collections in `value` deterministically relative to
        // this entry's (deterministic) id, so independently-created nested CRDTs
        // converge across nodes instead of carrying per-node random ids.
        super::rekey::rekey_nested_value(&mut value, id);

        if let Some(mut entry) = self.inner.get_mut(id)? {
            let (_, v) = &mut *entry;

            return Ok(Some(mem::replace(v, value)));
        }

        let _ignored = self
            .inner
            .insert_with_storage_type(Some(id), (key, value), storage_type)?;

        Ok(None)
    }

    /// Get the number of entries in the map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.inner.len()
    }

    /// Returns `true` if the map contains no entries.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }

    /// Get the value for a key in the map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    pub fn get<Q>(&self, key: &Q) -> Result<Option<V>, StoreError>
    where
        K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        let id = compute_id(self.inner.id(), key.as_ref());

        Ok(self.inner.get(id)?.map(|(_, v)| v))
    }

    /// Returns a mutable `ValueMut` guard for the value at `key`.
    ///
    /// Modifications are written back to storage when the guard is dropped.
    /// Mutating a value never changes the key set, so the ordering is unaffected.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    pub fn get_mut<'a, Q>(
        &'a mut self,
        key: &Q,
    ) -> Result<Option<ValueMut<'a, K, V, S>>, StoreError>
    where
        K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        let id = compute_id(self.inner.id(), key.as_ref());

        let entry_option = self.inner.get_mut(id)?;

        Ok(entry_option.map(|entry_mut| ValueMut { entry_mut }))
    }

    /// Gets the given key's corresponding entry for in-place manipulation.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn entry<'a>(&'a mut self, key: K) -> Result<Entry<'a, K, V, S>, StoreError>
    where
        K: PartialEq + AsRef<[u8]>,
    {
        let id = compute_id(self.inner.id(), key.as_ref());

        if self.inner.contains(id)? {
            let entry_mut = self
                .inner
                .get_mut(id)?
                .ok_or(StoreError::StorageError(StorageError::NotFound(id)))?;

            Ok(Entry::Occupied(OccupiedEntry { entry_mut }))
        } else {
            Ok(Entry::Vacant(VacantEntry { map: self, key }))
        }
    }

    /// Check if the map contains a key.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    pub fn contains<Q>(&self, key: &Q) -> Result<bool, StoreError>
    where
        K: Borrow<Q> + PartialEq,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        let id = compute_id(self.inner.id(), key.as_ref());

        self.inner.contains(id)
    }

    /// Remove a key from the map, returning the value if it previously existed.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    pub fn remove<Q>(&mut self, key: &Q) -> Result<Option<V>, StoreError>
    where
        K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        let id = compute_id(self.inner.id(), key.as_ref());

        let Some(entry) = self.inner.get_mut(id)? else {
            return Ok(None);
        };

        entry.remove().map(|(_, v)| Some(v))
    }

    /// Clear the map, removing all entries.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    pub fn clear(&mut self) -> Result<(), StoreError> {
        self.inner.clear()
    }

    /// Iterate entries in storage (hash) order — *not* key order.
    ///
    /// This is the building block the ordered readers sort. Kept private so the
    /// public surface only ever exposes key-ordered iteration; merge and
    /// migration paths that don't care about order use it to avoid the `K: Ord`
    /// bound.
    fn iter_unordered(&self) -> Result<impl Iterator<Item = (K, V)> + '_, StoreError> {
        let collection_id = self.inner.id();
        Ok(self.inner.entries()?.filter_map(move |result| match result {
            Ok(entry) => Some(entry),
            Err(error) => {
                tracing::error!(
                    target: "calimero_storage::iter_drop",
                    %collection_id,
                    %error,
                    collection_type = "SortedMap",
                    "ITER_DROP: parent's child list advertises an id whose entry could not be loaded — \
                     likely entry-before-parent ordering race or storage inconsistency. \
                     Caller will see a truncated iteration."
                );
                None
            }
        }).fuse())
    }
}

impl<K, V, S> SortedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + Ord,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Materialise every `(K, V)` pair sorted ascending by key.
    ///
    /// This is the single full scan that backs every ordered reader. See the
    /// [module docs](self) for why the order must be derived in memory rather
    /// than seeked.
    fn sorted_pairs(&self) -> Result<Vec<(K, V)>, StoreError> {
        let mut pairs: Vec<(K, V)> = self.iter_unordered()?.collect();
        pairs.sort_by(|(a, _), (b, _)| a.cmp(b));
        Ok(pairs)
    }

    /// Iterate all entries in ascending key order.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn entries(&self) -> Result<impl Iterator<Item = (K, V)>, StoreError> {
        Ok(self.sorted_pairs()?.into_iter())
    }

    /// Iterate all keys in ascending order.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn keys(&self) -> Result<impl Iterator<Item = K>, StoreError> {
        Ok(self.sorted_pairs()?.into_iter().map(|(k, _)| k))
    }

    /// Iterate all values in ascending key order.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn values(&self) -> Result<impl Iterator<Item = V>, StoreError> {
        Ok(self.sorted_pairs()?.into_iter().map(|(_, v)| v))
    }

    /// Iterate the entries whose keys fall within `range`, in ascending order.
    ///
    /// Accepts any [`RangeBounds<K>`], e.g. `a..b`, `a..=b`, `..b`, `c..`.
    ///
    /// # Example
    /// ```ignore
    /// // entries with keys in ["m", "t")
    /// for (k, v) in map.range("m".to_owned().."t".to_owned())? { /* ... */ }
    /// ```
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn range<R>(&self, range: R) -> Result<impl Iterator<Item = (K, V)>, StoreError>
    where
        R: RangeBounds<K>,
    {
        // Filter to the range *before* sorting so the sort cost scales with the
        // number of matches `m`, not the whole map `n`: O(n) scan + O(m log m).
        let mut pairs: Vec<(K, V)> = self
            .iter_unordered()?
            .filter(|(k, _)| range.contains(k))
            .collect();
        pairs.sort_by(|(a, _), (b, _)| a.cmp(b));
        Ok(pairs.into_iter())
    }

    /// Iterate the entries whose key bytes start with `prefix`, in ascending
    /// order.
    ///
    /// Useful for hierarchical keys such as `"user:"` or `"2026-05:"`. Relies on
    /// `K`'s byte representation ([`AsRef<[u8]>`]) matching its sort order — true
    /// for `String`/`&str` and other lexicographically-ordered byte keys.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn prefix(&self, prefix: &[u8]) -> Result<impl Iterator<Item = (K, V)>, StoreError>
    where
        K: AsRef<[u8]>,
    {
        // Filter by prefix before sorting: O(n) scan + O(m log m) on matches.
        let prefix = prefix.to_vec();
        let mut pairs: Vec<(K, V)> = self
            .iter_unordered()?
            .filter(|(k, _)| k.as_ref().starts_with(&prefix))
            .collect();
        pairs.sort_by(|(a, _), (b, _)| a.cmp(b));
        Ok(pairs.into_iter())
    }

    /// Return a page of `limit` entries starting at `offset`, in ascending key
    /// order. The canonical way to paginate without loading the whole map into
    /// the caller.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn page(&self, offset: usize, limit: usize) -> Result<Vec<(K, V)>, StoreError> {
        Ok(self
            .sorted_pairs()?
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect())
    }

    /// The entry with the smallest key, if any.
    ///
    /// A single O(n) min-pass — it does **not** sort the whole map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn first(&self) -> Result<Option<(K, V)>, StoreError> {
        Ok(self.iter_unordered()?.min_by(|(a, _), (b, _)| a.cmp(b)))
    }

    /// The entry with the largest key, if any.
    ///
    /// A single O(n) max-pass — it does **not** sort the whole map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn last(&self) -> Result<Option<(K, V)>, StoreError> {
        Ok(self.iter_unordered()?.max_by(|(a, _), (b, _)| a.cmp(b)))
    }
}

// Implement Data for SortedMap by delegating to its inner Collection.
impl<K, V, S> Data for SortedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        self.inner.collections()
    }

    fn element(&self) -> &Element {
        self.inner.element()
    }

    fn element_mut(&mut self) -> &mut Element {
        self.inner.element_mut()
    }
}

impl<K, V, S> Eq for SortedMap<K, V, S>
where
    K: Eq + Ord + BorshSerialize + BorshDeserialize,
    V: Eq + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
}

impl<K, V, S> PartialEq for SortedMap<K, V, S>
where
    K: Ord + BorshSerialize + BorshDeserialize,
    V: PartialEq + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    #[expect(clippy::unwrap_used, reason = "'tis fine")]
    fn eq(&self, other: &Self) -> bool {
        let l = self.entries().unwrap();
        let r = other.entries().unwrap();

        l.eq(r)
    }
}

impl<K, V, S> Ord for SortedMap<K, V, S>
where
    K: Ord + BorshSerialize + BorshDeserialize,
    V: Ord + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    #[expect(clippy::unwrap_used, reason = "'tis fine")]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let l = self.entries().unwrap();
        let r = other.entries().unwrap();

        l.cmp(r)
    }
}

impl<K, V, S> PartialOrd for SortedMap<K, V, S>
where
    K: Ord + BorshSerialize + BorshDeserialize,
    V: PartialOrd + Ord + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let l = self.entries().ok()?;
        let r = other.entries().ok()?;

        l.partial_cmp(r)
    }
}

impl<K, V, S> fmt::Debug for SortedMap<K, V, S>
where
    K: Ord + fmt::Debug + BorshSerialize + BorshDeserialize,
    V: fmt::Debug + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    #[expect(clippy::unwrap_used, clippy::unwrap_in_result, reason = "'tis fine")]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.debug_struct("SortedMap")
                .field("entries", &self.inner)
                .finish()
        } else {
            f.debug_map().entries(self.entries().unwrap()).finish()
        }
    }
}

impl<K, V, S> Default for SortedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn default() -> Self {
        Self::new_internal()
    }
}

impl<K, V, S> Serialize for SortedMap<K, V, S>
where
    K: Ord + BorshSerialize + BorshDeserialize + Serialize,
    V: BorshSerialize + BorshDeserialize + Serialize,
    S: StorageAdaptor,
{
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        let len = self.len().map_err(serde::ser::Error::custom)?;

        let mut seq = serializer.serialize_map(Some(len))?;

        // Entries are emitted in ascending key order — a `SortedMap` serialises
        // to a deterministically-ordered JSON object.
        for (k, v) in self.entries().map_err(serde::ser::Error::custom)? {
            seq.serialize_entry(&k, &v)?;
        }

        seq.end()
    }
}

impl<K, V, S> Extend<(K, V)> for SortedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq + 'static,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        // Register this map type's own re-key thunk, exactly as the other store
        // paths do, so a map populated only via `extend`/`collect` is still
        // re-keyed when it is itself nested as a value (map-of-map).
        super::rekey::register_rekey::<Self>();

        let parent = self.inner.id();

        let iter = iter.into_iter().map(|(k, mut v)| {
            let id = compute_id(parent, k.as_ref());

            // Re-key nested collections in the value relative to its entry id,
            // matching `insert`/`VacantEntry::insert`. Without this, a nested CRDT
            // bulk-inserted via `extend`/`collect` keeps a random internal id and
            // two nodes that independently build the same entry never converge.
            super::rekey::rekey_nested_value(&mut v, id);

            (Some(id), (k, v))
        });

        self.inner.extend(iter);
    }
}

impl<K, V, S> FromIterator<(K, V)> for SortedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq + 'static,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = SortedMap::new_internal();

        map.extend(iter);

        map
    }
}

/// A mutable guard for a value in a [`SortedMap`].
///
/// Changes are written back to storage when this guard is dropped.
#[derive(Debug)]
pub struct ValueMut<'a, K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    entry_mut: EntryMut<'a, (K, V), S>,
}

impl<K, V, S> Deref for ValueMut<'_, K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    type Target = V;

    fn deref(&self) -> &Self::Target {
        &self.entry_mut.deref().1
    }
}

impl<K, V, S> DerefMut for ValueMut<'_, K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entry_mut.deref_mut().1
    }
}

/// A view into a single entry in a [`SortedMap`], which may either be occupied
/// or vacant. Returned by [`SortedMap::entry`].
#[derive(Debug)]
pub enum Entry<'a, K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// An occupied entry.
    Occupied(OccupiedEntry<'a, K, V, S>),
    /// A vacant entry.
    Vacant(VacantEntry<'a, K, V, S>),
}

/// A view into an occupied entry in a [`SortedMap`].
#[derive(Debug)]
pub struct OccupiedEntry<'a, K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    entry_mut: EntryMut<'a, (K, V), S>,
}

/// A view into a vacant entry in a [`SortedMap`].
pub struct VacantEntry<'a, K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    map: &'a mut SortedMap<K, V, S>,
    key: K,
}

// Hand-written so it doesn't pull in `SortedMap: Debug` (which requires `K: Ord`
// for its sorted iteration). The vacant entry only meaningfully has a key.
impl<K, V, S> fmt::Debug for VacantEntry<'_, K, V, S>
where
    K: fmt::Debug + BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VacantEntry")
            .field("key", &self.key)
            .finish()
    }
}

impl<'a, K, V, S> Entry<'a, K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Ensures a value is in the entry by inserting the default if empty, and
    /// returns a mutable `ValueMut` guard to the value.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn or_insert(self, default: V) -> Result<ValueMut<'a, K, V, S>, StoreError>
    where
        V: 'static,
    {
        match self {
            Entry::Occupied(entry) => Ok(ValueMut {
                entry_mut: entry.entry_mut,
            }),
            Entry::Vacant(entry) => entry.insert(default),
        }
    }

    /// Ensures a value is in the entry by inserting the result of `f` if empty,
    /// and returns a mutable `ValueMut` guard to the value.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn or_insert_with<F>(self, f: F) -> Result<ValueMut<'a, K, V, S>, StoreError>
    where
        F: FnOnce() -> V,
        V: 'static,
    {
        match self {
            Entry::Occupied(entry) => Ok(ValueMut {
                entry_mut: entry.entry_mut,
            }),
            Entry::Vacant(entry) => entry.insert(f()),
        }
    }
}

impl<K, V, S> OccupiedEntry<'_, K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Gets a reference to the value in the entry.
    pub fn get(&self) -> &V {
        &self.entry_mut.1
    }

    /// Gets a mutable reference to the value in the entry.
    pub fn get_mut(&mut self) -> &mut V {
        &mut self.entry_mut.1
    }

    /// Replaces the value in the entry and returns the old value.
    pub fn insert(&mut self, mut value: V) -> V
    where
        V: 'static,
    {
        // Re-key nested collections in the replacement value relative to this
        // entry's (stable, deterministic) id — same reason as `VacantEntry::insert`.
        super::rekey::rekey_nested_value(&mut value, self.entry_mut.id());
        mem::replace(&mut self.entry_mut.1, value)
    }

    /// Removes the entry from the map and returns the removed value.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn remove(self) -> Result<V, StoreError> {
        self.entry_mut.remove().map(|(_, v)| v)
    }
}

impl<'a, K, V, S> VacantEntry<'a, K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Inserts a new value into the entry and returns a mutable `ValueMut`
    /// guard to the new value.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn insert(self, mut value: V) -> Result<ValueMut<'a, K, V, S>, StoreError>
    where
        V: 'static,
    {
        let id = compute_id(self.map.inner.id(), self.key.as_ref());

        // Re-key any nested collections in `value` deterministically relative to
        // this entry's (deterministic) id — exactly as `insert_with_storage_type`
        // does, so a nested CRDT stored via the Entry API converges across nodes.
        super::rekey::rekey_nested_value(&mut value, id);

        drop(self.map.inner.insert(Some(id), (self.key, value))?);

        let entry_mut = self
            .map
            .inner
            .get_mut(id)?
            .ok_or(StoreError::StorageError(StorageError::NotFound(id)))?;

        Ok(ValueMut { entry_mut })
    }
}

#[cfg(test)]
mod tests {
    use crate::collections::sorted_map::Entry;
    use crate::collections::{Root, SortedMap};
    use crate::store::MainStorage;

    #[test]
    fn test_sorted_map_basic_operations() {
        let mut map = Root::new(|| SortedMap::<_, _, MainStorage>::new());

        assert!(map
            .insert("key".to_owned(), "value".to_owned())
            .expect("insert failed")
            .is_none());

        assert_eq!(
            map.get("key").expect("get failed").as_deref(),
            Some("value")
        );

        assert_eq!(
            map.insert("key".to_owned(), "value2".to_owned())
                .expect("insert failed")
                .as_deref(),
            Some("value")
        );

        assert_eq!(
            map.remove("key")
                .expect("error while removing key")
                .as_deref(),
            Some("value2")
        );
        assert_eq!(map.get("key").expect("get failed"), None);
    }

    #[test]
    fn test_sorted_map_iterates_in_key_order_regardless_of_insertion_order() {
        let mut map = Root::new(|| SortedMap::<_, _, MainStorage>::new());

        // Insert deliberately out of order.
        for k in ["delta", "alpha", "charlie", "bravo", "echo"] {
            map.insert(k.to_owned(), k.to_uppercase()).unwrap();
        }

        let keys: Vec<String> = map.keys().expect("keys failed").collect();
        assert_eq!(keys, vec!["alpha", "bravo", "charlie", "delta", "echo"]);

        let entries: Vec<(String, String)> = map.entries().expect("entries failed").collect();
        assert_eq!(entries.first().unwrap().0, "alpha");
        assert_eq!(entries.last().unwrap().0, "echo");

        let values: Vec<String> = map.values().expect("values failed").collect();
        assert_eq!(values, vec!["ALPHA", "BRAVO", "CHARLIE", "DELTA", "ECHO"]);
    }

    #[test]
    fn test_sorted_map_range() {
        let mut map = Root::new(|| SortedMap::<_, _, MainStorage>::new());
        for k in ["a", "b", "c", "d", "e", "f"] {
            map.insert(k.to_owned(), k.to_owned()).unwrap();
        }

        // Half-open range [b, e).
        let got: Vec<String> = map
            .range("b".to_owned().."e".to_owned())
            .expect("range failed")
            .map(|(k, _)| k)
            .collect();
        assert_eq!(got, vec!["b", "c", "d"]);

        // Inclusive range [b, e].
        let got: Vec<String> = map
            .range("b".to_owned()..="e".to_owned())
            .expect("range failed")
            .map(|(k, _)| k)
            .collect();
        assert_eq!(got, vec!["b", "c", "d", "e"]);

        // Unbounded start ..c.
        let got: Vec<String> = map
            .range(.."c".to_owned())
            .expect("range failed")
            .map(|(k, _)| k)
            .collect();
        assert_eq!(got, vec!["a", "b"]);
    }

    #[test]
    fn test_sorted_map_prefix() {
        let mut map = Root::new(|| SortedMap::<_, _, MainStorage>::new());
        for k in ["user:alice", "user:bob", "post:1", "user:carol", "post:2"] {
            map.insert(k.to_owned(), String::new()).unwrap();
        }

        let users: Vec<String> = map
            .prefix(b"user:")
            .expect("prefix failed")
            .map(|(k, _)| k)
            .collect();
        assert_eq!(users, vec!["user:alice", "user:bob", "user:carol"]);

        let posts: Vec<String> = map
            .prefix(b"post:")
            .expect("prefix failed")
            .map(|(k, _)| k)
            .collect();
        assert_eq!(posts, vec!["post:1", "post:2"]);
    }

    #[test]
    fn test_sorted_map_pagination() {
        let mut map = Root::new(|| SortedMap::<_, _, MainStorage>::new());
        for i in 0..10u32 {
            // Zero-pad so lexicographic order matches numeric order.
            map.insert(format!("k{i:02}"), i).unwrap();
        }

        let page0 = map.page(0, 3).expect("page failed");
        assert_eq!(
            page0.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
            vec!["k00", "k01", "k02"]
        );

        let page1 = map.page(3, 3).expect("page failed");
        assert_eq!(
            page1.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
            vec!["k03", "k04", "k05"]
        );

        // Last partial page.
        let page3 = map.page(9, 3).expect("page failed");
        assert_eq!(page3.len(), 1);
        assert_eq!(page3[0].0, "k09");
    }

    #[test]
    fn test_sorted_map_first_last() {
        let mut map = Root::new(|| SortedMap::<_, _, MainStorage>::new());
        assert!(map.first().unwrap().is_none());
        assert!(map.last().unwrap().is_none());

        for k in ["m", "a", "z", "f"] {
            map.insert(k.to_owned(), k.to_owned()).unwrap();
        }

        assert_eq!(map.first().unwrap().unwrap().0, "a");
        assert_eq!(map.last().unwrap().unwrap().0, "z");
    }

    #[test]
    fn test_sorted_map_entry_api() {
        let mut map = Root::new(|| SortedMap::<_, _, MainStorage>::new());

        {
            let mut guard = map
                .entry("key1".to_owned())
                .expect("entry failed")
                .or_insert("value1".to_owned())
                .expect("or_insert failed");
            assert_eq!(*guard, "value1");
            *guard = "updated".to_owned();
        }
        assert_eq!(map.get("key1").unwrap().as_deref(), Some("updated"));

        // or_insert on occupied keeps the existing value.
        let guard = map
            .entry("key1".to_owned())
            .expect("entry failed")
            .or_insert("ignored".to_owned())
            .expect("or_insert failed");
        assert_eq!(*guard, "updated");
    }

    #[test]
    fn test_sorted_map_remove_updates_order() {
        let mut map = Root::new(|| SortedMap::<_, _, MainStorage>::new());
        for k in ["a", "b", "c", "d"] {
            map.insert(k.to_owned(), k.to_owned()).unwrap();
        }

        drop(map.remove("b").unwrap());

        let keys: Vec<String> = map.keys().unwrap().collect();
        assert_eq!(keys, vec!["a", "c", "d"]);
        assert_eq!(map.len().unwrap(), 3);
    }

    #[test]
    fn test_sorted_map_get_mut_preserves_order() {
        let mut map = Root::new(|| SortedMap::<_, _, MainStorage>::new());
        for k in ["b", "a", "c"] {
            map.insert(k.to_owned(), 0u32).unwrap();
        }

        {
            let mut guard = map.get_mut("a").unwrap().expect("key not found");
            *guard = 42;
        }

        let entries: Vec<(String, u32)> = map.entries().unwrap().collect();
        assert_eq!(
            entries,
            vec![
                ("a".to_owned(), 42),
                ("b".to_owned(), 0),
                ("c".to_owned(), 0)
            ]
        );
    }

    #[test]
    fn test_deterministic_sorted_map_ids() {
        crate::env::reset_for_testing();

        let map1: SortedMap<String, String> = SortedMap::new_with_field_name("items");
        let map2: SortedMap<String, String> = SortedMap::new_with_field_name("items");

        assert_eq!(
            <SortedMap<String, String> as crate::entities::Data>::id(&map1),
            <SortedMap<String, String> as crate::entities::Data>::id(&map2),
            "Maps with same field name should have same ID"
        );

        let map3: SortedMap<String, String> = SortedMap::new_with_field_name("products");
        assert_ne!(
            <SortedMap<String, String> as crate::entities::Data>::id(&map1),
            <SortedMap<String, String> as crate::entities::Data>::id(&map3),
            "Maps with different field names should have different IDs"
        );
    }

    #[test]
    fn test_reassign_deterministic_id_preserves_sorted_entries() {
        crate::env::reset_for_testing();

        let mut map = SortedMap::<String, String>::new();
        map.insert("beta".to_owned(), "two".to_owned())
            .expect("insert beta failed");
        map.insert("alpha".to_owned(), "one".to_owned())
            .expect("insert alpha failed");

        let old_id = <SortedMap<String, String> as crate::entities::Data>::id(&map);
        map.reassign_deterministic_id("items");
        let new_id = <SortedMap<String, String> as crate::entities::Data>::id(&map);

        assert_ne!(old_id, new_id);
        let keys: Vec<String> = map.keys().expect("keys failed").collect();
        assert_eq!(keys, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_entry_occupied_is_used_in_match() {
        let mut map = Root::new(|| SortedMap::<_, _, MainStorage>::new());
        map.insert("k".to_owned(), "v".to_owned()).unwrap();

        let value = match map.entry("k".to_owned()).unwrap() {
            Entry::Occupied(e) => e.get().clone(),
            Entry::Vacant(_) => panic!("expected occupied"),
        };
        assert_eq!(value, "v");
    }
}
