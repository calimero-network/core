//! This module provides functionality for the unordered map data structure.

use core::borrow::Borrow;
use core::fmt;
use std::mem;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::ser::SerializeMap;
use serde::Serialize;

use super::{compute_id, Collection, StorageAdaptor};
use crate::collections::error::StoreError;
use crate::entities::Data;
use crate::store::MainStorage;

/// A map collection that stores key-value pairs.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct UnorderedMap<K, V, S: StorageAdaptor = MainStorage> {
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: Collection<(K, V), S>,
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
        let id = compute_id(self.inner.id(), key.as_ref());

        if let Some(mut entry) = self.inner.get_mut(id)? {
            let (_, v) = &mut *entry;

            return Ok(Some(mem::replace(v, value)));
        }

        let _ignored = self.inner.insert(Some(id), (key, value))?;

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

#[cfg(test)]
mod tests {
    use crate::collections::{Root, UnorderedMap};

    #[test]
    fn test_unordered_map_basic_operations() {
        let mut map = Root::new(|| UnorderedMap::new());

        assert!(map
            .insert("key".to_string(), "value".to_string())
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
            map.insert("key".to_string(), "value2".to_string())
                .expect("insert failed")
                .as_deref(),
            Some("value")
        );
        assert!(map
            .insert("key2".to_string(), "value".to_string())
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
            .insert("key1".to_string(), "value1".to_string())
            .expect("insert failed")
            .is_none());
        assert!(map
            .insert("key2".to_string(), "value2".to_string())
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
            .insert("key".to_string(), "value".to_string())
            .expect("insert failed")
            .is_none());
        assert!(!map
            .insert("key".to_string(), "new_value".to_string())
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
            .insert("key".to_string(), "value".to_string())
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
            .insert("key1".to_string(), "value1".to_string())
            .expect("insert failed")
            .is_none());
        assert!(map
            .insert("key2".to_string(), "value2".to_string())
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
            .insert("key1".to_string(), "value1".to_string())
            .expect("insert failed")
            .is_none());
        assert!(map
            .insert("key2".to_string(), "value2".to_string())
            .expect("insert failed")
            .is_none());
        assert!(!map
            .insert("key2".to_string(), "value3".to_string())
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
            .insert("key".to_string(), "value".to_string())
            .expect("insert failed")
            .is_none());

        assert_eq!(map.contains("key").expect("contains failed"), true);
        assert_eq!(map.contains("nonexistent").expect("contains failed"), false);
    }

    #[test]
    fn test_unordered_map_entries() {
        let mut map = Root::new(|| UnorderedMap::new());

        assert!(map
            .insert("key1".to_string(), "value1".to_string())
            .expect("insert failed")
            .is_none());
        assert!(map
            .insert("key2".to_string(), "value2".to_string())
            .expect("insert failed")
            .is_none());
        assert!(!map
            .insert("key2".to_string(), "value3".to_string())
            .expect("insert failed")
            .is_none());

        let entries: Vec<(String, String)> = map.entries().expect("entries failed").collect();

        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&("key1".to_string(), "value1".to_string())));
        assert!(entries.contains(&("key2".to_string(), "value3".to_string())));
    }
}
