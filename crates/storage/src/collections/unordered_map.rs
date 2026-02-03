//! This module provides functionality for the unordered map data structure.

use core::borrow::Borrow;
use core::fmt;
use core::ops::{Deref, DerefMut};
use std::mem;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::ser::SerializeMap;
use serde::Serialize;

use super::{compute_id, Collection, EntryMut, StorageAdaptor};
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
    pub(crate) inner: Collection<(K, V), S>,
}

impl<K, V> UnorderedMap<K, V, MainStorage>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
{
    /// Create a new map collection.
    pub fn new() -> Self {
        Self::new_internal()
    }

    /// Create a new map collection with field name for schema inference.
    ///
    /// This enables merodb and other tools to infer the schema from the database
    /// without requiring an external schema file. The field name is used to
    /// generate deterministic collection IDs.
    pub fn new_with_field_name(field_name: &str) -> Self {
        Self {
            inner: Collection::new_with_field_name(None, field_name),
        }
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
        K: AsRef<[u8]> + PartialEq,
    {
        // By default, add as the Public storage.
        self.insert_with_storage_type(key, value, StorageType::Public, None)
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
        value: V,
        storage_type: StorageType,
        custom_id: Option<Id>,
    ) -> Result<Option<V>, StoreError>
    where
        K: AsRef<[u8]> + PartialEq,
    {
        let id = custom_id.unwrap_or_else(|| compute_id(self.inner.id(), key.as_ref()));

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
        Ok(self.inner.entries()?.flatten().fuse())
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

    /// Get the value for a key in the map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn get<Q>(&self, key: &Q) -> Result<Option<V>, StoreError>
    where
        K: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        let id = compute_id(self.inner.id(), key.as_ref());

        Ok(self.inner.get(id)?.map(|(_, v)| v))
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

            Ok(Entry::Occupied(OccupiedEntry { entry_mut }))
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
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn default() -> Self {
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
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        let parent = self.inner.id();

        let iter = iter.into_iter().map(|(k, v)| {
            let id = compute_id(parent, k.as_ref());

            (Some(id), (k, v))
        });

        self.inner.extend(iter);
    }
}

impl<K, V, S> FromIterator<(K, V)> for UnorderedMap<K, V, S>
where
    K: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    V: BorshSerialize + BorshDeserialize,
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
    pub fn or_insert(self, default: V) -> Result<ValueMut<'a, K, V, S>, StoreError> {
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
    ///
    /// Changes are written back to storage when the returned `DerefMut`
    /// guard is dropped.
    pub fn get_mut(&mut self) -> &mut V {
        &mut self.entry_mut.1
    }

    /// Replaces the value in the entry and returns the old value.
    pub fn insert(&mut self, value: V) -> V {
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
    pub fn insert(self, value: V) -> Result<ValueMut<'a, K, V, S>, StoreError> {
        let id = compute_id(self.map.inner.id(), self.key.as_ref());

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

    #[test]
    fn test_unordered_map_basic_operations() {
        let mut map = Root::new(|| UnorderedMap::new());

        assert!(map
            .insert("key".to_owned(), "value".to_owned())
            .expect("insert failed")
            .is_none());

        assert_eq!(
            map.get("key").expect("get failed").as_deref(),
            Some("value")
        );
        assert_ne!(
            map.get("key").expect("get failed").as_deref(),
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
            map.get("key").expect("get failed").as_deref(),
            Some("value2")
        );
        assert_eq!(
            map.get("key2").expect("get failed").as_deref(),
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
        let mut map = Root::new(|| UnorderedMap::new());

        assert!(map
            .insert("key1".to_owned(), "value1".to_owned())
            .expect("insert failed")
            .is_none());
        assert!(map
            .insert("key2".to_owned(), "value2".to_owned())
            .expect("insert failed")
            .is_none());

        assert_eq!(
            map.get("key1").expect("get failed").as_deref(),
            Some("value1")
        );
        assert_eq!(
            map.get("key2").expect("get failed").as_deref(),
            Some("value2")
        );
    }

    #[test]
    fn test_unordered_map_update_value() {
        let mut map = Root::new(|| UnorderedMap::new());

        assert!(map
            .insert("key".to_owned(), "value".to_owned())
            .expect("insert failed")
            .is_none());
        assert!(!map
            .insert("key".to_owned(), "new_value".to_owned())
            .expect("insert failed")
            .is_none());

        assert_eq!(
            map.get("key").expect("get failed").as_deref(),
            Some("new_value")
        );
    }

    #[test]
    fn test_remove() {
        let mut map = Root::new(|| UnorderedMap::new());

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
        let mut map = Root::new(|| UnorderedMap::new());

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
        let mut map = Root::new(|| UnorderedMap::new());

        assert_eq!(map.len().expect("len failed"), 0);

        assert!(map
            .insert("key1".to_owned(), "value1".to_owned())
            .expect("insert failed")
            .is_none());
        assert!(map
            .insert("key2".to_owned(), "value2".to_owned())
            .expect("insert failed")
            .is_none());
        assert!(!map
            .insert("key2".to_owned(), "value3".to_owned())
            .expect("insert failed")
            .is_none());

        assert_eq!(map.len().expect("len failed"), 2);

        assert_eq!(
            map.remove("key1").expect("remove failed").as_deref(),
            Some("value1")
        );

        assert_eq!(map.len().expect("len failed"), 1);
    }

    #[test]
    fn test_unordered_map_contains() {
        let mut map = Root::new(|| UnorderedMap::new());

        assert!(map
            .insert("key".to_owned(), "value".to_owned())
            .expect("insert failed")
            .is_none());

        assert_eq!(map.contains("key").expect("contains failed"), true);
        assert_eq!(map.contains("nonexistent").expect("contains failed"), false);
    }

    #[test]
    fn test_unordered_map_entries() {
        let mut map = Root::new(|| UnorderedMap::new());

        assert!(map
            .insert("key1".to_owned(), "value1".to_owned())
            .expect("insert failed")
            .is_none());
        assert!(map
            .insert("key2".to_owned(), "value2".to_owned())
            .expect("insert failed")
            .is_none());
        assert!(!map
            .insert("key2".to_owned(), "value3".to_owned())
            .expect("insert failed")
            .is_none());

        let entries: Vec<(String, String)> = map.entries().expect("entries failed").collect();

        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&("key1".to_owned(), "value1".to_owned())));
        assert!(entries.contains(&("key2".to_owned(), "value3".to_owned())));
    }

    #[test]
    fn test_unordered_map_get_mut() {
        let mut map = Root::new(|| UnorderedMap::new());
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
            map.get("key1").expect("get failed").as_deref(),
            Some("new_value")
        );

        // Try to get a non-existent key
        let guard = map.get_mut("key_nonexistent").expect("get_mut failed");
        assert!(guard.is_none());
    }

    #[test]
    fn test_unordered_map_entry_vacant() {
        let mut map = Root::new(|| UnorderedMap::new());

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
        assert_eq!(map.get("key1").unwrap().as_deref(), Some("new_value1"));

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
        assert_eq!(map.get("key2").unwrap().as_deref(), Some("value2"));
    }

    #[test]
    fn test_unordered_map_entry_occupied_or_insert() {
        let mut map = Root::new(|| UnorderedMap::new());
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
        assert_eq!(map.get("key1").unwrap().as_deref(), Some("value1"));

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

        assert_eq!(called, false); // Verify closure was not executed
        assert_eq!(map.len().unwrap(), 1);
        assert_eq!(map.get("key1").unwrap().as_deref(), Some("value1"));
    }

    #[test]
    fn test_unordered_map_entry_occupied_mutations() {
        let mut map = Root::new(|| UnorderedMap::new());
        drop(map.insert("key1".to_owned(), "value1".to_owned()).unwrap());
        drop(map.insert("key2".to_owned(), "value2".to_owned()).unwrap());
        drop(map.insert("key3".to_owned(), "value3".to_owned()).unwrap());

        // Test `OccupiedEntry::get_mut()`
        if let Ok(Entry::Occupied(mut entry)) = map.entry("key1".to_owned()) {
            *entry.get_mut() = "updated_value1".to_owned();
        } else {
            panic!("Entry should be occupied");
        }
        assert_eq!(map.get("key1").unwrap().as_deref(), Some("updated_value1"));

        // Test `OccupiedEntry::insert()`
        let old_val = if let Ok(Entry::Occupied(mut entry)) = map.entry("key2".to_owned()) {
            entry.insert("updated_value2".to_owned())
        } else {
            panic!("Entry should be occupied");
        };
        assert_eq!(old_val, "value2");
        assert_eq!(map.get("key2").unwrap().as_deref(), Some("updated_value2"));
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
}
