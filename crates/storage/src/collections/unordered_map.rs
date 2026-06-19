//! This module provides functionality for the unordered map data structure.
//!
//! [`UnorderedMap`] is the **default** key-value collection: point-lookup and
//! full-scan only, with **no iteration-order guarantee** (entries come back in
//! hashed entity-id order, not key order). Add-wins CRDT merge — keys union,
//! shared keys merge their values recursively.
//!
//! # Complexity (on a node)
//!
//! | Operation | Cost |
//! |---|---|
//! | `get` / `insert` / `remove` / `contains` | `O(1)` point lookup |
//! | `len` | `O(1)` |
//! | `entries` / `keys` / `values` | `O(n)`, **unordered** |
//!
//! There is no separate index to maintain, so writes are as cheap as the
//! storage engine allows and no extra disk is used per key.
//!
//! # `UnorderedMap` vs [`SortedMap`](super::SortedMap)
//!
//! **Default to `UnorderedMap`.** Reach for [`SortedMap`](super::SortedMap)
//! *only* when you need keys in order — `range(a..b)`, `prefix("user:")`,
//! pagination, sorted iteration, or min/max. `SortedMap` answers those in
//! `O(log n + k)` via a maintained on-disk index, but pays for it on every
//! write (an extra index write + a validity-marker read/write), in extra disk
//! per key, and with an `O(n)` index rebuild on the first ordered read after a
//! sync. If you only ever point-access a map, that index is pure overhead — use
//! `UnorderedMap`. It is the `HashMap` to `SortedMap`'s `BTreeMap`.

use core::borrow::Borrow;
use core::fmt;
use core::ops::{Deref, DerefMut};
use std::mem;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::ser::SerializeMap;
use serde::Serialize;

use super::{compute_id, Collection, CrdtType, EntryMut, StorageAdaptor, StorageKey, ValueRef};
use crate::address::Id;
use crate::collections::error::StoreError;
use crate::entities::{ChildInfo, Data, Element, StorageType};
use crate::error::StorageError;
use crate::store::MainStorage;
use std::collections::BTreeMap;

/// A map collection that stores key-value pairs.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct UnorderedMap<K, V, S: StorageAdaptor = MainStorage> {
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: Collection<(K, V), S>,
}

/// Re-key a nested map (a map stored as another collection's value) relative to
/// its storage parent. `reassign_deterministic_id_under` re-inserts entries,
/// which recurses the re-key into each entry's own nested collections. See
/// [`super::rekey`].
impl<K, V, S> super::rekey::RekeyTarget for UnorderedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq + 'static,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    fn rekey_relative_to(&mut self, parent_id: Id) {
        self.reassign_deterministic_id_under(
            parent_id,
            "__nested_map",
            CrdtType::unordered_map(std::any::type_name::<K>(), std::any::type_name::<V>()),
        );
    }
}

impl<K, V, S> UnorderedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Create a new map collection with a random ID.
    ///
    /// This is the right constructor in both common cases:
    ///
    /// - **Top-level `#[app::state]` fields.** `new()` is sufficient — the macro
    ///   runs `__assign_deterministic_ids()` after `init()`/`migrate()` returns,
    ///   which calls `reassign_deterministic_id("<field>")` using the struct field
    ///   name. That derives the same deterministic ID `new_with_field_name("<field>")`
    ///   would, so the random ID minted here is replaced before any sync. Prefer
    ///   `new()` over `new_with_field_name` for these fields: the latter repeats the
    ///   field name as a string literal that must match exactly, and a typo silently
    ///   assigns the wrong ID (the entity then diverges across nodes with no error).
    /// - **Nested collections** stored as values in other maps. Merge happens by the
    ///   parent map's key, so the nested collection's random ID is fine as-is.
    ///
    /// `new_with_field_name` is only needed to assign a deterministic ID outside the
    /// macro pass (e.g. a collection constructed and used entirely within one call,
    /// before the post-init pass can reach it).
    ///
    /// The storage adaptor `S` is inferred from the binding context.
    /// Default-generic remains `MainStorage`, so existing call sites
    /// (`UnorderedMap::<K, V>::new()`) keep their behaviour. Inside a
    /// `#[app::private]` struct, the macro substitutes `PrivateStorage`
    /// as `S` on the field type, and this constructor infers `S =
    /// PrivateStorage` from the assignment site.
    pub fn new() -> Self {
        Self::new_internal()
    }

    /// Create a new map collection with a deterministic ID.
    ///
    /// The `field_name` is used to generate a deterministic collection ID,
    /// ensuring the same code produces the same ID across all nodes.
    ///
    /// Use this for top-level state fields (the `#[app::state]` macro does this
    /// automatically).
    ///
    /// # Example
    /// ```ignore
    /// let items = UnorderedMap::<String, String>::new_with_field_name("items");
    /// ```
    pub fn new_with_field_name(field_name: &str) -> Self {
        Self::new_with_field_name_internal(None, field_name)
    }
}

impl<K, V, S> UnorderedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Create a new map collection (internal, shared with Counter).
    pub(super) fn new_internal() -> Self {
        Self {
            inner: Collection::new(None),
        }
    }

    /// Creates a detached map that is NOT registered with the storage system.
    ///
    /// This is used for placeholder fields that are never actually used, such as
    /// GCounter's negative map which exists only to satisfy the type signature but
    /// is never persisted or read from storage.
    ///
    /// WARNING: Maps created with this method will NOT be synced across nodes.
    /// Only use this for truly inert placeholder fields.
    pub(super) fn new_detached() -> Self {
        Self {
            inner: Collection::new_detached(),
        }
    }

    /// Open a handle to an `UnorderedMap` that already exists in storage at
    /// `id`, without creating or re-registering it.
    ///
    /// Used by the interface layer to read/write a map whose id is computed
    /// out-of-band and whose parent linkage is owned by the caller (the
    /// rotation-log map under a `Shared` anchor — core#2716 P3). The element is
    /// stamped `CrdtType::UnorderedMap` so the merge dispatch and the per-entry
    /// children behave identically to a map created via `new_with_field_name`.
    pub(crate) fn open_existing(id: crate::address::Id) -> Self {
        Self {
            inner: Collection::open_existing(
                id,
                CrdtType::unordered_map(std::any::type_name::<K>(), std::any::type_name::<V>()),
            ),
        }
    }

    /// Create a new map collection with deterministic ID (internal)
    pub(super) fn new_with_field_name_internal(
        parent_id: Option<crate::address::Id>,
        field_name: &str,
    ) -> Self {
        Self {
            inner: Collection::new_with_field_name_and_crdt_type(
                parent_id,
                field_name,
                CrdtType::unordered_map(std::any::type_name::<K>(), std::any::type_name::<V>()),
            ),
        }
    }

    /// Create a new map with deterministic ID and explicit CRDT type (for Counter's internal maps)
    pub(super) fn new_with_field_name_and_crdt_type(
        parent_id: Option<crate::address::Id>,
        field_name: &str,
        crdt_type: CrdtType,
    ) -> Self {
        Self {
            inner: Collection::new_with_field_name_and_crdt_type(parent_id, field_name, crdt_type),
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
    /// `insert`'s re-key, gets a new deterministic id under the new parent).
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
        // Carry each entry's `StorageType` so per-entry owner stamps
        // (AuthoredMap / guarded Shared writers) survive the re-key; a plain
        // `(K, V)` snapshot would re-insert as `Public` and silently strip
        // ownership.
        let entries: Vec<((K, V), StorageType)> = self
            .inner
            .entries_with_storage_type()
            .expect("failed to read entries for re-key");

        // Clear the collection (removes old entries with old IDs).
        self.inner.clear().expect("failed to clear for re-key");

        // Reassign the collection's ID (Collection's `_with_crdt_type` is itself
        // just `_under(None, ..)`, so this single call covers both variants).
        self.inner
            .reassign_deterministic_id_under(parent_id, field_name, crdt_type);

        // Re-insert all entries (they will get new IDs based on new parent ID),
        // preserving each entry's original `StorageType`.
        for ((key, value), storage_type) in entries {
            self.insert_with_storage_type(key, value, storage_type, None)
                .expect("failed to re-insert entry during re-key");
        }
    }

    /// Reassigns the map's ID and collection CRDT type to deterministic values.
    ///
    /// This is used by wrappers like RGA that store map entries but expose a
    /// custom collection-level CRDT type.
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
    /// relative to `parent_id` (for a map nested inside another entity). Entries
    /// are re-inserted under the new parent id, so they (and their own nested
    /// values, via `insert`'s re-key) stay reachable and deterministic.
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

    /// Reassigns the map's ID to a deterministic ID based on field name.
    ///
    /// This is called by the `#[app::state]` macro after `init()` returns to ensure
    /// all top-level collections have deterministic IDs regardless of how they were
    /// created in `init()`.
    ///
    /// This method also migrates all existing entries to use the new parent ID,
    /// ensuring that entries inserted during `init()` remain accessible.
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
            CrdtType::unordered_map(std::any::type_name::<K>(), std::any::type_name::<V>()),
        );
    }

    /// Insert a key-value pair into the map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn insert(&mut self, key: K, value: V) -> Result<Option<V>, StoreError>
    where
        K: StorageKey,
        V: 'static,
    {
        // Children inherit this collection's own storage domain. For an ordinary
        // map the collection element is `Public`, so this is identical to the
        // previous hardcoded default. When the collection element carries
        // `Shared{writers}` (a guarded collection — e.g. the value of a
        // `SharedStorage`), every entry is stamped with that same writer set, so
        // the whole subtree is guarded at merge instead of only the wrapper.
        let inherited = self.inner.element().metadata.storage_type.clone();
        self.insert_with_storage_type(key, value, inherited, None)
    }

    /// Insert a key-value pair into the map with the specified `StorageType`.
    /// It also optionally allows passing a custom `Id`.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
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

        // Insert into the inner collection.
        // Pass the `StorageType` directly to the `Collection`.
        let _ignored = self
            .inner
            .insert_with_storage_type(Some(id), (key, value), storage_type)?;

        Ok(None)
    }

    /// Get an iterator over the entries in the map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn entries(&self) -> Result<impl Iterator<Item = (K, V)> + '_, StoreError> {
        // ITER_DROP diagnostic: the inner iterator yields `Result<…>`;
        // an `Err` means the parent's children list advertises an id
        // whose entry can't be loaded (e.g. `NotFound` from a
        // partially-written ancestor or an orphan from a divergent
        // sync). Skipping silently is the established CRDT-iter
        // contract (`.flatten().fuse()` did this), but logging gives
        // future races a precise anchor instead of a downstream
        // content mismatch.
        let collection_id = self.inner.id();
        Ok(self.inner.entries()?.filter_map(move |result| match result {
            Ok(entry) => Some(entry),
            Err(error) => {
                tracing::error!(
                    target: "calimero_storage::iter_drop",
                    %collection_id,
                    %error,
                    collection_type = "UnorderedMap",
                    "ITER_DROP: parent's child list advertises an id whose entry could not be loaded — \
                     likely entry-before-parent ordering race or storage inconsistency. \
                     Caller will see a truncated iteration."
                );
                None
            }
        }).fuse())
    }

    /// Get the number of entries in the map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
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

    /// Get the value for a key in the map.
    ///
    /// Returns a read-only [`ValueRef`] guard (an owned, deserialized copy that
    /// derefs to `&V`). Reads work transparently through it; to *mutate* the
    /// stored value use [`get_mut`](Self::get_mut) or
    /// [`entry`](Self::entry)`().or_default()` (both write back automatically),
    /// or `.clone()` the guard for an owned copy when `V: Clone`.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn get<Q>(&self, key: &Q) -> Result<Option<ValueRef<V>>, StoreError>
    where
        K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        let id = compute_id(self.inner.id(), key.as_ref());

        Ok(self.inner.get(id)?.map(|(_, v)| ValueRef::new(v)))
    }

    /// Returns a mutable reference to the value corresponding to the key.
    ///
    /// This returns a `ValueMut` guard. Any modifications to the value
    /// will be written back to storage when the guard is dropped.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn get_mut<'a, Q>(
        &'a mut self,
        key: &Q,
    ) -> Result<Option<ValueMut<'a, K, V, S>>, StoreError>
    where
        K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        let id = compute_id(self.inner.id(), key.as_ref());

        // Get the internal EntryMut<'a, (K, V), S>
        let entry_option = self.inner.get_mut(id)?;

        // Wrap it in ValueMut guard.
        // This guard only allows access to V.
        Ok(entry_option.map(|entry_mut| ValueMut { entry_mut }))
    }

    /// Gets the given key's corresponding entry in the map for in-place manipulation.
    ///
    /// This method returns a `Result` as it must access storage to check
    /// for the key's existence.
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

        // 1. First, check for existence using `contains`
        if self.inner.contains(id)? {
            // 2. If it exists, we can now safely get the mutable guard.
            // We `expect` because we literally just confirmed it exists.
            let entry_mut = self
                .inner
                .get_mut(id)?
                .ok_or(StoreError::StorageError(StorageError::NotFound(id)))?;

            Ok(Entry::Occupied(Box::new(OccupiedEntry { entry_mut })))
        } else {
            // 3. If it doesn't exist, no `EntryMut` was created.
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
    ///
    pub fn contains<Q>(&self, key: &Q) -> Result<bool, StoreError>
    where
        K: Borrow<Q> + PartialEq,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        let id = compute_id(self.inner.id(), key.as_ref());

        self.inner.contains(id)
    }

    /// Remove a key from the map, returning the value at the key if it previously existed.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
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
    ///
    pub fn clear(&mut self) -> Result<(), StoreError> {
        self.inner.clear()
    }
}

// Implement Data for UnorderedMap by delegating to its inner Collection
impl<K, V, S> Data for UnorderedMap<K, V, S>
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

impl<K, V, S> Eq for UnorderedMap<K, V, S>
where
    K: Eq + BorshSerialize + BorshDeserialize,
    V: Eq + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
}

impl<K, V, S> PartialEq for UnorderedMap<K, V, S>
where
    K: PartialEq + BorshSerialize + BorshDeserialize,
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

impl<K, V, S> Ord for UnorderedMap<K, V, S>
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

impl<K, V, S> PartialOrd for UnorderedMap<K, V, S>
where
    K: PartialOrd + BorshSerialize + BorshDeserialize,
    V: PartialOrd + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let l = self.entries().ok()?;
        let r = other.entries().ok()?;

        l.partial_cmp(r)
    }
}

impl<K, V, S> fmt::Debug for UnorderedMap<K, V, S>
where
    K: fmt::Debug + BorshSerialize + BorshDeserialize,
    V: fmt::Debug + BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    #[expect(clippy::unwrap_used, clippy::unwrap_in_result, reason = "'tis fine")]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.debug_struct("UnorderedMap")
                .field("entries", &self.inner)
                .finish()
        } else {
            f.debug_map().entries(self.entries().unwrap()).finish()
        }
    }
}

impl<K, V, S> Default for UnorderedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq + 'static,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    fn default() -> Self {
        // Register this map type's nested-id re-key thunk at construction, so a
        // map first created via `default()` — e.g. `entry(k).or_default()` on a
        // `Map<_, Map<..>>` — is known to the re-key registry BEFORE its parent's
        // `VacantEntry::insert` re-keys it. Without this the freshly-defaulted
        // inner collection keeps a random id and the two nodes that first-touch
        // it never converge. Mirrors `Counter::new_internal`'s registration.
        super::rekey::register_rekey::<Self>();
        Self::new_internal()
    }
}

impl<K, V, S> Serialize for UnorderedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + Serialize,
    V: BorshSerialize + BorshDeserialize + Serialize,
    S: StorageAdaptor,
{
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        let len = self.len().map_err(serde::ser::Error::custom)?;

        let mut seq = serializer.serialize_map(Some(len))?;

        for (k, v) in self.entries().map_err(serde::ser::Error::custom)? {
            seq.serialize_entry(&k, &v)?;
        }

        seq.end()
    }
}

impl<K, V, S> Extend<(K, V)> for UnorderedMap<K, V, S>
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

impl<K, V, S> FromIterator<(K, V)> for UnorderedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq + 'static,
    V: BorshSerialize + BorshDeserialize + 'static,
    S: StorageAdaptor,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut map = UnorderedMap::new_internal();

        map.extend(iter);

        map
    }
}

/// A mutable guard for a value in an `UnorderedMap`.
///
/// Changes are written back to storage when this guard is dropped.
#[derive(Debug)]
pub struct ValueMut<'a, K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// This holds the mutable entry for the *entire* tuple.
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
        // self.entry_mut.deref() returns &(K, V), so with .1 it accesses the V
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
        // self.entry_mut.deref() returns &(K, V), so with .1 it accesses the V
        &mut self.entry_mut.deref_mut().1
    }
}

/// A view into a single entry in an `UnorderedMap`, which may either be
/// occupied or vacant.
///
/// This `enum` is returned by the `UnorderedMap::entry` method.
#[must_use = "an Entry does nothing on its own; call `.or_insert(…)` / `.or_default()` \
              (or match it) to read or modify the slot — dropping it is a no-op"]
#[derive(Debug)]
pub enum Entry<'a, K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// An occupied entry.
    Occupied(Box<OccupiedEntry<'a, K, V, S>>),
    /// A vacant entry.
    Vacant(VacantEntry<'a, K, V, S>),
}

/// A view into an occupied entry in an `UnorderedMap`.
/// It holds a mutable guard to the entry.
#[derive(Debug)]
pub struct OccupiedEntry<'a, K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    entry_mut: EntryMut<'a, (K, V), S>,
}

/// A view into a vacant entry in an `UnorderedMap`.
/// It holds a mutable reference to the map and the key.
#[derive(Debug)]
pub struct VacantEntry<'a, K, V, S>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    map: &'a mut UnorderedMap<K, V, S>,
    key: K,
}

impl<'a, K, V, S> Entry<'a, K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Ensures a value is in the entry by inserting the default if empty,
    /// and returns a mutable `ValueMut` guard to the value.
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

    /// Ensures a value is in the entry by inserting the result of the
    /// function `f` if empty, and returns a mutable `ValueMut` guard to the value.
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

    /// Ensures a value is in the entry by inserting `V::default()` if empty,
    /// and returns a mutable `ValueMut` guard to the value.
    ///
    /// This is the blessed path for in-place mutation of nested CRDT values:
    /// the returned guard re-persists the value to storage when it is dropped,
    /// so there is no manual get → modify → re-insert dance. It also composes
    /// for nested collections — `map.entry(k)?.or_default()?` yields a guard
    /// whose nested CRDTs are deterministically re-keyed under this entry's id,
    /// so independently first-created values converge across nodes.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    pub fn or_default(self) -> Result<ValueMut<'a, K, V, S>, StoreError>
    where
        V: Default + 'static,
    {
        self.or_insert_with(V::default)
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
    ///
    /// Changes are written back to storage when the returned `DerefMut`
    /// guard is dropped.
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
        // Replacing an occupied entry with a freshly-built nested CRDT would
        // otherwise leave it carrying a random internal id that diverges across
        // nodes.
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
        // does. Without this, a nested CRDT stored via the Entry API
        // (`entry(k).or_insert(Counter::new())`) keeps the random internal id it
        // was minted with, so two nodes that independently first-create it never
        // converge. See `super::rekey`.
        super::rekey::rekey_nested_value(&mut value, id);

        // Insert the new (key, value) pair
        drop(self.map.inner.insert(Some(id), (self.key, value))?);

        // Now, get a mutable guard to the new entry.
        // We `expect` here because this is a logic error: we just inserted
        // an entry, so it MUST be found.
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
    use crate::collections::unordered_map::Entry;
    use crate::collections::{Root, UnorderedMap};
    use crate::store::MainStorage;

    #[test]
    fn test_unordered_map_basic_operations() {
        let mut map = Root::new(UnorderedMap::<_, _, MainStorage>::new);

        assert!(map
            .insert("key".to_owned(), "value".to_owned())
            .expect("insert failed")
            .is_none());

        assert_eq!(
            map.get("key")
                .expect("get failed")
                .as_deref()
                .map(String::as_str),
            Some("value")
        );
        assert_ne!(
            map.get("key")
                .expect("get failed")
                .as_deref()
                .map(String::as_str),
            Some("value2")
        );

        assert_eq!(
            map.insert("key".to_owned(), "value2".to_owned())
                .expect("insert failed")
                .as_deref(),
            Some("value")
        );
        assert!(map
            .insert("key2".to_owned(), "value".to_owned())
            .expect("insert failed")
            .is_none());

        assert_eq!(
            map.get("key")
                .expect("get failed")
                .as_deref()
                .map(String::as_str),
            Some("value2")
        );
        assert_eq!(
            map.get("key2")
                .expect("get failed")
                .as_deref()
                .map(String::as_str),
            Some("value")
        );

        assert_eq!(
            map.remove("key")
                .expect("error while removing key")
                .as_deref(),
            Some("value2")
        );
        assert_eq!(map.remove("key").expect("error while removing key"), None);

        assert_eq!(map.get("key").expect("get failed"), None);
    }

    #[test]
    fn test_unordered_map_insert_and_get() {
        let mut map = Root::new(UnorderedMap::<_, _, MainStorage>::new);

        assert!(map
            .insert("key1".to_owned(), "value1".to_owned())
            .expect("insert failed")
            .is_none());
        assert!(map
            .insert("key2".to_owned(), "value2".to_owned())
            .expect("insert failed")
            .is_none());

        assert_eq!(
            map.get("key1")
                .expect("get failed")
                .as_deref()
                .map(String::as_str),
            Some("value1")
        );
        assert_eq!(
            map.get("key2")
                .expect("get failed")
                .as_deref()
                .map(String::as_str),
            Some("value2")
        );
    }

    #[test]
    fn test_unordered_map_update_value() {
        let mut map = Root::new(UnorderedMap::<_, _, MainStorage>::new);

        assert!(map
            .insert("key".to_owned(), "value".to_owned())
            .expect("insert failed")
            .is_none());
        assert!(map
            .insert("key".to_owned(), "new_value".to_owned())
            .expect("insert failed")
            .is_some());

        assert_eq!(
            map.get("key")
                .expect("get failed")
                .as_deref()
                .map(String::as_str),
            Some("new_value")
        );
    }

    #[test]
    fn test_remove() {
        let mut map = Root::new(UnorderedMap::<_, _, MainStorage>::new);

        assert!(map
            .insert("key".to_owned(), "value".to_owned())
            .expect("insert failed")
            .is_none());

        assert_eq!(
            map.remove("key").expect("remove failed").as_deref(),
            Some("value")
        );
        assert_eq!(map.get("key").expect("get failed"), None);
    }

    #[test]
    fn test_clear() {
        let mut map = Root::new(UnorderedMap::<_, _, MainStorage>::new);

        assert!(map
            .insert("key1".to_owned(), "value1".to_owned())
            .expect("insert failed")
            .is_none());
        assert!(map
            .insert("key2".to_owned(), "value2".to_owned())
            .expect("insert failed")
            .is_none());

        map.clear().expect("clear failed");

        assert_eq!(map.get("key1").expect("get failed"), None);
        assert_eq!(map.get("key2").expect("get failed"), None);
    }

    #[test]
    fn test_unordered_map_len() {
        let mut map = Root::new(UnorderedMap::<_, _, MainStorage>::new);

        assert_eq!(map.len().expect("len failed"), 0);

        assert!(map
            .insert("key1".to_owned(), "value1".to_owned())
            .expect("insert failed")
            .is_none());
        assert!(map
            .insert("key2".to_owned(), "value2".to_owned())
            .expect("insert failed")
            .is_none());
        assert!(map
            .insert("key2".to_owned(), "value3".to_owned())
            .expect("insert failed")
            .is_some());

        assert_eq!(map.len().expect("len failed"), 2);

        assert_eq!(
            map.remove("key1").expect("remove failed").as_deref(),
            Some("value1")
        );

        assert_eq!(map.len().expect("len failed"), 1);
    }

    #[test]
    fn test_unordered_map_contains() {
        let mut map = Root::new(UnorderedMap::<_, _, MainStorage>::new);

        assert!(map
            .insert("key".to_owned(), "value".to_owned())
            .expect("insert failed")
            .is_none());

        assert!(map.contains("key").expect("contains failed"));
        assert!(!map.contains("nonexistent").expect("contains failed"));
    }

    #[test]
    fn test_unordered_map_entries() {
        let mut map = Root::new(UnorderedMap::<_, _, MainStorage>::new);

        assert!(map
            .insert("key1".to_owned(), "value1".to_owned())
            .expect("insert failed")
            .is_none());
        assert!(map
            .insert("key2".to_owned(), "value2".to_owned())
            .expect("insert failed")
            .is_none());
        assert!(map
            .insert("key2".to_owned(), "value3".to_owned())
            .expect("insert failed")
            .is_some());

        let entries: Vec<(String, String)> = map.entries().expect("entries failed").collect();

        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&("key1".to_owned(), "value1".to_owned())));
        assert!(entries.contains(&("key2".to_owned(), "value3".to_owned())));
    }

    #[test]
    fn test_unordered_map_get_mut() {
        let mut map = Root::new(UnorderedMap::<_, _, MainStorage>::new);
        drop(
            map.insert("key1".to_owned(), "value1".to_owned())
                .expect("insert failed"),
        );

        // Get and modify an existing key
        {
            let mut guard = map
                .get_mut("key1")
                .expect("get_mut failed")
                .expect("key not found");
            assert_eq!(*guard, "value1");

            // Modify the value via the guard
            *guard = "new_value".to_owned();
        } // Guard is dropped here, change is committed

        // Verify the change was persisted
        assert_eq!(
            map.get("key1")
                .expect("get failed")
                .as_deref()
                .map(String::as_str),
            Some("new_value")
        );

        // Try to get a non-existent key
        let guard = map.get_mut("key_nonexistent").expect("get_mut failed");
        assert!(guard.is_none());
    }

    #[test]
    fn test_unordered_map_entry_vacant() {
        let mut map = Root::new(UnorderedMap::<_, _, MainStorage>::new);

        // Test `or_insert()`
        {
            let mut guard = map
                .entry("key1".to_owned())
                .expect("entry failed")
                .or_insert("value1".to_owned())
                .expect("or_insert failed");

            // The guard points to the new value
            assert_eq!(*guard, "value1");
            *guard = "new_value1".to_owned();
        } // Guard is dropped, "new_value1" is committed

        assert_eq!(map.len().unwrap(), 1);
        assert_eq!(
            map.get("key1").unwrap().as_deref().map(String::as_str),
            Some("new_value1")
        );

        // Test `or_insert_with()`
        {
            let guard = map
                .entry("key2".to_owned())
                .expect("entry failed")
                .or_insert_with(|| "value2".to_owned())
                .expect("or_insert_with failed");

            assert_eq!(*guard, "value2");
        } // Guard is dropped

        assert_eq!(map.len().unwrap(), 2);
        assert_eq!(
            map.get("key2").unwrap().as_deref().map(String::as_str),
            Some("value2")
        );
    }

    #[test]
    fn test_unordered_map_entry_occupied_or_insert() {
        let mut map = Root::new(UnorderedMap::<_, _, MainStorage>::new);
        drop(
            map.insert("key1".to_owned(), "value1".to_owned())
                .expect("insert failed"),
        );

        // Test `or_insert()` on an occupied entry
        {
            let guard = map
                .entry("key1".to_owned())
                .expect("entry failed")
                .or_insert("new_value".to_owned()) // This value should be ignored
                .expect("or_insert failed");

            // Guard should point to the *original* value
            assert_eq!(*guard, "value1");
        } // Guard is dropped

        // Make sure the value hasn't changed
        assert_eq!(map.len().unwrap(), 1);
        assert_eq!(
            map.get("key1").unwrap().as_deref().map(String::as_str),
            Some("value1")
        );

        // Test `or_insert_with()` on an occupied entry
        let mut called = false;
        {
            let guard = map
                .entry("key1".to_owned())
                .expect("entry failed")
                .or_insert_with(|| {
                    // This closure should not be called
                    called = true;
                    "new_value".to_owned()
                })
                .expect("or_insert_with failed");

            assert_eq!(*guard, "value1");
        } // Guard is dropped

        assert!(!called); // Verify closure was not executed
        assert_eq!(map.len().unwrap(), 1);
        assert_eq!(
            map.get("key1").unwrap().as_deref().map(String::as_str),
            Some("value1")
        );
    }

    #[test]
    fn test_unordered_map_entry_or_default() {
        let mut map = Root::new(UnorderedMap::<String, u64, MainStorage>::new);

        // Vacant: `or_default()` inserts `u64::default()` (0) and the write-back
        // guard persists the mutation made through it on drop.
        {
            let mut guard = map
                .entry("key1".to_owned())
                .expect("entry failed")
                .or_default()
                .expect("or_default failed");
            assert_eq!(*guard, 0);
            *guard += 5;
        } // Guard is dropped -> value re-persisted

        assert_eq!(map.get("key1").unwrap().as_deref().copied(), Some(5));
        assert_eq!(map.len().unwrap(), 1);

        // Occupied: `or_default()` yields the existing value, not a fresh default.
        {
            let mut guard = map
                .entry("key1".to_owned())
                .expect("entry failed")
                .or_default()
                .expect("or_default failed");
            assert_eq!(*guard, 5);
            *guard += 1;
        } // Guard is dropped -> value re-persisted

        assert_eq!(map.get("key1").unwrap().as_deref().copied(), Some(6));
        assert_eq!(map.len().unwrap(), 1);
    }

    #[test]
    fn test_unordered_map_entry_occupied_mutations() {
        let mut map = Root::new(UnorderedMap::<_, _, MainStorage>::new);
        drop(map.insert("key1".to_owned(), "value1".to_owned()).unwrap());
        drop(map.insert("key2".to_owned(), "value2".to_owned()).unwrap());
        drop(map.insert("key3".to_owned(), "value3".to_owned()).unwrap());

        // Test `OccupiedEntry::get_mut()`
        if let Ok(Entry::Occupied(mut entry)) = map.entry("key1".to_owned()) {
            *entry.get_mut() = "updated_value1".to_owned();
        } else {
            panic!("Entry should be occupied");
        }
        assert_eq!(
            map.get("key1").unwrap().as_deref().map(String::as_str),
            Some("updated_value1")
        );

        // Test `OccupiedEntry::insert()`
        let old_val = if let Ok(Entry::Occupied(mut entry)) = map.entry("key2".to_owned()) {
            entry.insert("updated_value2".to_owned())
        } else {
            panic!("Entry should be occupied");
        };
        assert_eq!(old_val, "value2");
        assert_eq!(
            map.get("key2").unwrap().as_deref().map(String::as_str),
            Some("updated_value2")
        );
        assert_eq!(map.len().unwrap(), 3); // Length should be unchanged

        // Test `OccupiedEntry::remove()`
        let old_val = if let Ok(Entry::Occupied(entry)) = map.entry("key3".to_owned()) {
            entry.remove().expect("remove failed")
        } else {
            panic!("Entry should be occupied");
        };
        assert_eq!(old_val, "value3");
        assert_eq!(map.get("key3").unwrap(), None); // Key should be gone
        assert_eq!(map.len().unwrap(), 2); // Length should decrease
    }

    #[test]
    fn insert_inherits_collection_storage_domain() {
        use std::collections::BTreeSet;

        use calimero_primitives::identity::PublicKey;

        use crate::address::Id;
        use crate::collections::compute_id;
        use crate::entities::{Data, StorageType};
        use crate::interface::Interface;
        use crate::store::MainStorage;

        crate::env::reset_for_testing();

        // Load a map entry's stored entity and return the StorageType it was
        // stamped with. `crate::collections::Entry` is the storage entry type
        // (distinct from this module's public `Entry` API enum).
        fn child_storage_type(map_id: Id, key: &str) -> StorageType {
            let child = compute_id(map_id, key.as_bytes());
            let entry = <Interface<MainStorage>>::find_by_id::<
                crate::collections::Entry<(String, String)>,
            >(child)
            .expect("load child entry")
            .expect("child entry exists");
            entry.storage.metadata.storage_type
        }

        // Ordinary map: entries stay Public — no behaviour change.
        let mut public_map = UnorderedMap::<String, String>::new();
        let _ignored = public_map
            .insert("k".to_owned(), "v".to_owned())
            .expect("insert into public map");
        let public_id = <UnorderedMap<String, String> as Data>::id(&public_map);
        assert!(
            matches!(child_storage_type(public_id, "k"), StorageType::Public),
            "ordinary map entries must remain Public",
        );

        // Guarded map: stamping the collection's own element `Shared{writers}`
        // propagates that domain to every entry, so the whole subtree is guarded
        // at merge — not just the wrapper. This is the core of guarding a
        // collection by a writer set.
        let mut guarded = UnorderedMap::<String, String>::new();
        let writers: BTreeSet<PublicKey> = std::iter::once(PublicKey::from([7u8; 32])).collect();
        guarded.element_mut().set_shared_domain(writers.clone());
        let _ignored = guarded
            .insert("k".to_owned(), "v".to_owned())
            .expect("insert into guarded map");
        let guarded_id = <UnorderedMap<String, String> as Data>::id(&guarded);
        match child_storage_type(guarded_id, "k") {
            StorageType::Shared { writers: w, .. } => {
                assert_eq!(w, crate::entities::full_mask(writers.clone()))
            }
            other => panic!("guarded map entry must inherit Shared, got {other:?}"),
        }
    }

    #[test]
    fn entry_or_default_inherits_collection_storage_domain() {
        use std::collections::BTreeSet;

        use calimero_primitives::identity::PublicKey;

        use crate::address::Id;
        use crate::collections::compute_id;
        use crate::entities::{Data, StorageType};
        use crate::interface::Interface;
        use crate::store::MainStorage;

        crate::env::reset_for_testing();

        fn child_storage_type(map_id: Id, key: &str) -> StorageType {
            let child = compute_id(map_id, key.as_bytes());
            let entry = <Interface<MainStorage>>::find_by_id::<
                crate::collections::Entry<(String, String)>,
            >(child)
            .expect("load child entry")
            .expect("child entry exists");
            entry.storage.metadata.storage_type
        }

        // Guard the collection, then create an entry through the Entry/or_default
        // write-back path (a different write path than `map.insert`). It must
        // inherit the domain too, otherwise guarding silently fails for the
        // blessed nested-CRDT mutation API.
        let mut guarded = UnorderedMap::<String, String>::new();
        let writers: BTreeSet<PublicKey> = std::iter::once(PublicKey::from([7u8; 32])).collect();
        guarded.element_mut().set_shared_domain(writers.clone());
        {
            let mut value = guarded
                .entry("k".to_owned())
                .expect("entry")
                .or_default()
                .expect("or_default");
            *value = "v".to_owned();
        }

        let guarded_id = <UnorderedMap<String, String> as Data>::id(&guarded);
        match child_storage_type(guarded_id, "k") {
            StorageType::Shared { writers: w, .. } => {
                assert_eq!(w, crate::entities::full_mask(writers.clone()))
            }
            other => panic!("entry/or_default entry must inherit Shared, got {other:?}"),
        }
    }

    #[test]
    fn test_deterministic_map_ids() {
        crate::env::reset_for_testing();

        // Create two maps with the same field name - they should have the same IDs
        let map1_val: UnorderedMap<String, String> = UnorderedMap::new_with_field_name("items");
        let map2_val: UnorderedMap<String, String> = UnorderedMap::new_with_field_name("items");

        assert_eq!(
            <UnorderedMap<String, String> as crate::entities::Data>::id(&map1_val),
            <UnorderedMap<String, String> as crate::entities::Data>::id(&map2_val),
            "Maps with same field name should have same ID"
        );

        // Different field names should produce different IDs
        let map3_val: UnorderedMap<String, String> = UnorderedMap::new_with_field_name("products");
        assert_ne!(
            <UnorderedMap<String, String> as crate::entities::Data>::id(&map1_val),
            <UnorderedMap<String, String> as crate::entities::Data>::id(&map3_val),
            "Maps with different field names should have different IDs"
        );
    }

    #[test]
    fn test_random_vs_deterministic_map_ids() {
        crate::env::reset_for_testing();

        // Random IDs (new()) should be different each time
        let map1: UnorderedMap<String, String> = UnorderedMap::new();
        let map2: UnorderedMap<String, String> = UnorderedMap::new();

        assert_ne!(
            <UnorderedMap<String, String> as crate::entities::Data>::id(&map1),
            <UnorderedMap<String, String> as crate::entities::Data>::id(&map2),
            "Maps with new() should have different random IDs"
        );

        // Deterministic IDs (new_with_field_name) should be the same
        let map3: UnorderedMap<String, String> = UnorderedMap::new_with_field_name("items");
        let map4: UnorderedMap<String, String> = UnorderedMap::new_with_field_name("items");
        assert_eq!(
            <UnorderedMap<String, String> as crate::entities::Data>::id(&map3),
            <UnorderedMap<String, String> as crate::entities::Data>::id(&map4),
            "Maps with same field name should have same ID"
        );
    }

    #[test]
    fn test_new_plus_reassign_matches_new_with_field_name() {
        // Safety lock for dropping `new_with_field_name("x")` in favour of plain
        // `::new()`: the conversion is only sound because the `#[app::state]`
        // post-init pass calls `reassign_deterministic_id("x")`, which MUST derive
        // the identical deterministic id `new_with_field_name("x")` produces. If
        // these two id derivations ever drift apart, every app that switched to
        // `::new()` would silently mint a different id on creation and split-brain
        // across nodes (CIP I9) with no compile error — so pin them equal here.
        crate::env::reset_for_testing();

        let explicit: UnorderedMap<String, String> = UnorderedMap::new_with_field_name("items");

        let mut via_pass: UnorderedMap<String, String> = UnorderedMap::new();
        via_pass.reassign_deterministic_id("items");

        assert_eq!(
            <UnorderedMap<String, String> as crate::entities::Data>::id(&explicit),
            <UnorderedMap<String, String> as crate::entities::Data>::id(&via_pass),
            "new() + post-init reassign must produce the same id as new_with_field_name",
        );
    }

    #[test]
    fn test_reassign_deterministic_id_preserves_entries() {
        crate::env::reset_for_testing();

        let mut map = UnorderedMap::<String, String>::new();
        map.insert("alpha".to_owned(), "one".to_owned())
            .expect("insert alpha failed");
        map.insert("beta".to_owned(), "two".to_owned())
            .expect("insert beta failed");

        let old_id = <UnorderedMap<String, String> as crate::entities::Data>::id(&map);
        map.reassign_deterministic_id("items");
        let new_id = <UnorderedMap<String, String> as crate::entities::Data>::id(&map);

        assert_ne!(
            old_id, new_id,
            "Map ID should change after deterministic reassignment"
        );
        assert_eq!(
            map.get("alpha")
                .expect("get alpha failed")
                .as_deref()
                .map(String::as_str),
            Some("one"),
            "alpha entry should survive reassignment"
        );
        assert_eq!(
            map.get("beta")
                .expect("get beta failed")
                .as_deref()
                .map(String::as_str),
            Some("two"),
            "beta entry should survive reassignment"
        );
        assert_eq!(map.len().expect("len failed"), 2);
    }

    #[test]
    fn test_nested_map_uses_random_ids() {
        crate::env::reset_for_testing();

        // For nested maps (values in other maps), we use new() which generates random IDs.
        // This is fine because merge happens by the parent map's key, not the nested map's ID.
        let mut parent_map = Root::new(UnorderedMap::<String, String>::new);

        // Insert an entry
        parent_map
            .insert("key".to_string(), "value".to_string())
            .unwrap();

        // Verify the entry exists
        assert!(
            parent_map.contains("key").unwrap(),
            "Map entry should exist"
        );

        // Nested maps created with new() have random IDs - this is intentional
        // because merge happens by key, not by the nested map's ID
        let nested1: UnorderedMap<String, String> = UnorderedMap::new();
        let nested2: UnorderedMap<String, String> = UnorderedMap::new();

        assert_ne!(
            <UnorderedMap<String, String> as crate::entities::Data>::id(&nested1),
            <UnorderedMap<String, String> as crate::entities::Data>::id(&nested2),
            "Nested maps with new() should have different IDs (random)"
        );
    }
}
