//! This module provides functionality for the unordered set data structure.

use core::borrow::Borrow;

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

use super::Collection;
use crate::address::Id;
use crate::collections::error::StoreError;
use crate::entities::Data;

/// A set collection that stores unqiue values once.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub struct UnorderedSet<V> {
    inner: Collection<V>,
}

impl<V> UnorderedSet<V>
where
    V: BorshSerialize + BorshDeserialize,
{
    /// Create a new set collection.
    ///
    pub fn new() -> Self {
        Self {
            inner: Collection::new(None),
        }
    }

    /// Compute the ID for a value in the set.
    fn compute_id(&self, value: &[u8]) -> Id {
        let mut hasher = Sha256::new();
        hasher.update(self.inner.id().as_bytes());
        hasher.update(value);
        Id::new(hasher.finalize().into())
    }

    /// Insert a value pair into the set collection if the element does not already exist.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn insert(&mut self, value: V) -> Result<bool, StoreError>
    where
        V: AsRef<[u8]> + PartialEq,
    {
        let id = self.compute_id(value.as_ref());

        if self.inner.get_mut(id)?.is_some() {
            return Ok(false);
        };

        let _ignored = self.inner.insert(Some(id), value)?;

        Ok(true)
    }

    /// Get an iterator over the entries in the set.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn entries(&self) -> Result<impl Iterator<Item = V> + '_, StoreError> {
        Ok(self.inner.entries()?.flatten().fuse())
    }

    /// Get the number of entries in the set.
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

    /// Get the value for a key in the set.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn contains<Q>(&self, value: &Q) -> Result<bool, StoreError>
    where
        V: Borrow<Q>,
        Q: PartialEq + ?Sized + AsRef<[u8]>,
    {
        let id = self.compute_id(value.as_ref());

        Ok(self.inner.get(id)?.is_some())
    }

    /// Remove a key from the set, returning the value at the key if it previously existed.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn remove<Q>(&mut self, value: &Q) -> Result<bool, StoreError>
    where
        V: Borrow<Q>,
        Q: PartialEq + AsRef<[u8]> + ?Sized,
    {
        let id = self.compute_id(value.as_ref());

        let Some(entry) = self.inner.get_mut(id)? else {
            return Ok(false);
        };

        let _ignored = entry.remove()?;

        Ok(true)
    }

    /// Clear the set, removing all entries.
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

impl<V> Eq for UnorderedSet<V> where V: Eq + BorshSerialize + BorshDeserialize {}

impl<V> PartialEq for UnorderedSet<V>
where
    V: PartialEq + BorshSerialize + BorshDeserialize,
{
    fn eq(&self, other: &Self) -> bool {
        self.entries().unwrap().eq(other.entries().unwrap())
    }
}

impl<V> Ord for UnorderedSet<V>
where
    V: Ord + BorshSerialize + BorshDeserialize,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.entries().unwrap().cmp(other.entries().unwrap())
    }
}

impl<V> PartialOrd for UnorderedSet<V>
where
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
    use crate::collections::{Root, UnorderedSet};

    #[test]
    fn test_unordered_set_operations() {
        let _root = Root::new(());

        let mut set = UnorderedSet::<String>::new();

        assert!(set.insert("value1".to_string()).expect("insert failed"));

        assert_eq!(
            set.contains(&"value1".to_string())
                .expect("contains failed"),
            true
        );

        assert!(!set.insert("value1".to_string()).expect("insert failed"));
        assert!(set.insert("value2".to_string()).expect("insert failed"));

        assert_eq!(set.contains("value3").expect("get failed"), false);
        assert_eq!(set.contains("value2").expect("get failed"), true);

        assert_eq!(
            set.remove("value1").expect("error while removing key"),
            true
        );
        assert_eq!(
            set.remove("value3").expect("error while removing key"),
            false
        );
    }

    #[test]
    fn test_unordered_set_len() {
        let _root = Root::new(());

        let mut set = UnorderedSet::<String>::new();

        assert!(set.insert("value1".to_string()).expect("insert failed"));
        assert!(set.insert("value2".to_string()).expect("insert failed"));
        assert!(!set.insert("value2".to_string()).expect("insert failed"));

        assert_eq!(set.len().expect("len failed"), 2);

        assert!(set.remove("value1").expect("remove failed"));

        assert_eq!(set.len().expect("len failed"), 1);
    }

    #[test]
    fn test_unordered_set_clear() {
        let _root = Root::new(());

        let mut set = UnorderedSet::<String>::new();

        assert!(set.insert("value1".to_string()).expect("insert failed"));
        assert!(set.insert("value2".to_string()).expect("insert failed"));

        assert_eq!(set.len().expect("len failed"), 2);

        set.clear().expect("clear failed");

        assert_eq!(set.len().expect("len failed"), 0);
        assert_eq!(set.contains("value1").expect("contains failed"), false);
        assert_eq!(set.contains("value2").expect("contains failed"), false);
    }

    #[test]
    fn test_unordered_set_entries() {
        let _root = Root::new(());

        let mut set = UnorderedSet::<String>::new();

        assert!(set.insert("value1".to_string()).expect("insert failed"));
        assert!(set.insert("value2".to_string()).expect("insert failed"));

        let entries: Vec<String> = set.entries().expect("entries failed").collect();

        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&"value1".to_string()));
        assert!(entries.contains(&"value2".to_string()));

        assert!(set.remove("value1").expect("remove failed"));
        let entries: Vec<String> = set.entries().expect("entries failed").collect();
        assert_eq!(entries.len(), 1);
    }
}
