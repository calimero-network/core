//! This module provides functionality for the unordered map data structure.

use core::borrow::Borrow;
use std::mem;

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

use super::Collection;
use crate::address::Id;
use crate::collections::error::StoreError;
use crate::entities::Data;

/// A map collection that stores key-value pairs.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct UnorderedMap<K, V> {
    inner: Collection<(K, V)>,
}

impl<K, V> UnorderedMap<K, V>
where
    K: BorshSerialize + BorshDeserialize,
    V: BorshSerialize + BorshDeserialize,
{
    /// Create a new map collection.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn new() -> Self {
        Self {
            inner: Collection::new(),
        }
    }

    /// Compute the ID for a key.
    fn compute_id(&self, key: &[u8]) -> Id {
        let mut hasher = Sha256::new();
        hasher.update(self.inner.id().as_bytes());
        hasher.update(key);
        Id::new(hasher.finalize().into())
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
        let id = self.compute_id(key.as_ref());

        if let Some(mut entry) = self.inner.get_mut(id)? {
            let (_, v) = &mut *entry;

            return Ok(Some(mem::replace(v, value)));
        }

        self.inner.insert(Some(id), (key, value))?;

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
        let iter = self.inner.entries()?;

        let iter = iter.flat_map(|entry| entry.ok());

        Ok(iter.fuse())
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
        Ok(self.inner.entries()?.len())
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
        let id = self.compute_id(key.as_ref());

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
        self.get(key).map(|v| v.is_some())
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
        let id = self.compute_id(key.as_ref());

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

impl<K, V> Eq for UnorderedMap<K, V>
where
    K: Eq + BorshSerialize + BorshDeserialize,
    V: Eq + BorshSerialize + BorshDeserialize,
{
}

impl<K, V> PartialEq for UnorderedMap<K, V>
where
    K: PartialEq + BorshSerialize + BorshDeserialize,
    V: PartialEq + BorshSerialize + BorshDeserialize,
{
    fn eq(&self, other: &Self) -> bool {
        self.entries().unwrap().eq(other.entries().unwrap())
    }
}

impl<K, V> Ord for UnorderedMap<K, V>
where
    K: Ord + BorshSerialize + BorshDeserialize,
    V: Ord + BorshSerialize + BorshDeserialize,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.entries().unwrap().cmp(other.entries().unwrap())
    }
}

impl<K, V> PartialOrd for UnorderedMap<K, V>
where
    K: PartialOrd + BorshSerialize + BorshDeserialize,
    V: PartialOrd + BorshSerialize + BorshDeserialize,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.entries()
            .unwrap()
            .partial_cmp(other.entries().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use crate::collections::unordered_map::UnorderedMap;

    #[test]
    fn test_unordered_map_basic_operations() {
        let mut map = UnorderedMap::<String, String>::new();

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
        let mut map = UnorderedMap::<String, String>::new();

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
        let mut map = UnorderedMap::<String, String>::new();

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
        let mut map = UnorderedMap::<String, String>::new();

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
        let mut map = UnorderedMap::<String, String>::new();

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
        let mut map = UnorderedMap::<String, String>::new();

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
        let mut map = UnorderedMap::<String, String>::new();

        assert!(map
            .insert("key".to_string(), "value".to_string())
            .expect("insert failed")
            .is_none());

        assert_eq!(map.contains("key").expect("contains failed"), true);
        assert_eq!(map.contains("nonexistent").expect("contains failed"), false);
    }

    #[test]
    fn test_unordered_map_entries() {
        let mut map = UnorderedMap::<String, String>::new();

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
