//! This module provides functionality for the vector data structure.

use core::borrow::Borrow;
use core::fmt;
use std::mem;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::ser::SerializeSeq;
use serde::Serialize;

use super::Collection;
use crate::collections::error::StoreError;
use crate::store::{MainStorage, StorageAdaptor};

/// A vector collection that stores key-value pairs.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct Vector<V, S: StorageAdaptor = MainStorage> {
    // Borrow/ToOwned
    #[borsh(bound(serialize = "", deserialize = ""))]
    inner: Collection<V, S>,
}

impl<V> Vector<V, MainStorage>
where
    V: BorshSerialize + BorshDeserialize,
{
    /// Create a new vector collection.
    pub fn new() -> Self {
        Self::new_internal()
    }
}

impl<V, S> Vector<V, S>
where
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Create a new vector collection (internal, shared with decompose).
    pub(super) fn new_internal() -> Self {
        use super::CrdtType;
        Self {
            inner: Collection::new_with_crdt_type(None, CrdtType::Vector),
        }
    }

    /// Add a value to the end of the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn push(&mut self, value: V) -> Result<(), StoreError> {
        let _ignored = self.inner.insert(None, value)?;

        Ok(())
    }

    /// Remove and return the last value from the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn pop(&mut self) -> Result<Option<V>, StoreError> {
        let Some(last) = self.inner.last()? else {
            return Ok(None);
        };

        let Some(entry) = self.inner.get_mut(last)? else {
            return Ok(None);
        };

        let last = entry.remove()?;

        Ok(Some(last))
    }

    /// Get the value at a specific index in the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn get(&self, index: usize) -> Result<Option<V>, StoreError> {
        self.inner.entries()?.nth(index).transpose()
    }

    /// Update the value at a specific index in the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn update(&mut self, index: usize, value: V) -> Result<Option<V>, StoreError> {
        let Some(id) = self.inner.nth(index)? else {
            return Ok(None);
        };

        let Some(mut entry) = self.inner.get_mut(id)? else {
            return Ok(None);
        };

        let old = mem::replace(&mut *entry, value);

        Ok(Some(old))
    }

    /// Get an iterator over the items in the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn iter(&self) -> Result<impl Iterator<Item = V> + '_, StoreError> {
        Ok(self.inner.entries()?.flatten().fuse())
    }

    /// Get the last value in the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn last(&self) -> Result<Option<V>, StoreError> {
        let Some(last) = self.inner.last()? else {
            return Ok(None);
        };

        self.inner.get(last)
    }

    /// Get the number of items in the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    #[expect(clippy::len_without_is_empty, reason = "TODO: will be implemented")]
    pub fn len(&self) -> Result<usize, StoreError> {
        self.inner.len()
    }

    /// Get the value for a key in the vector.
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
        Q: PartialEq,
    {
        for entry in self.iter()? {
            if value == entry.borrow() {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Clear the vector, removing all items.
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

    /// Find the first element that matches the predicate and return an iterator to it.
    ///
    /// Returns an iterator that yields at most one element (the first match).
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn find<F>(&self, mut predicate: F) -> Result<impl Iterator<Item = V>, StoreError>
    where
        F: FnMut(&V) -> bool,
    {
        let found = self.iter()?.find(|item| predicate(item));
        Ok(found.into_iter())
    }

    /// Filter elements that match the predicate and return an iterator over all matches.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn filter<'a, F>(&'a self, predicate: F) -> Result<impl Iterator<Item = V> + 'a, StoreError>
    where
        F: FnMut(&V) -> bool + 'a,
    {
        Ok(self.iter()?.filter(predicate))
    }
}

impl<V> Eq for Vector<V> where V: Eq + BorshSerialize + BorshDeserialize {}

impl<V> PartialEq for Vector<V>
where
    V: PartialEq + BorshSerialize + BorshDeserialize,
{
    #[expect(clippy::unwrap_used, reason = "'tis fine")]
    fn eq(&self, other: &Self) -> bool {
        let l = self.iter().unwrap();
        let r = other.iter().unwrap();

        l.eq(r)
    }
}

impl<V> Ord for Vector<V>
where
    V: Ord + BorshSerialize + BorshDeserialize,
{
    #[expect(clippy::unwrap_used, reason = "'tis fine")]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let l = self.iter().unwrap();
        let r = other.iter().unwrap();

        l.cmp(r)
    }
}

impl<V> PartialOrd for Vector<V>
where
    V: PartialOrd + BorshSerialize + BorshDeserialize,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let l = self.iter().ok()?;
        let r = other.iter().ok()?;

        l.partial_cmp(r)
    }
}

impl<V> fmt::Debug for Vector<V>
where
    V: fmt::Debug + BorshSerialize + BorshDeserialize,
{
    #[expect(clippy::unwrap_used, clippy::unwrap_in_result, reason = "'tis fine")]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.debug_struct("Vector")
                .field("items", &self.inner)
                .finish()
        } else {
            f.debug_list().entries(self.iter().unwrap()).finish()
        }
    }
}

impl<V, S> Default for Vector<V, S>
where
    V: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn default() -> Self {
        Self::new_internal()
    }
}

impl<V, S> Serialize for Vector<V, S>
where
    V: BorshSerialize + BorshDeserialize + Serialize,
    S: StorageAdaptor,
{
    fn serialize<Ser>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error>
    where
        Ser: serde::Serializer,
    {
        let len = self.len().map_err(serde::ser::Error::custom)?;

        let mut seq = serializer.serialize_seq(Some(len))?;

        for entry in self.iter().map_err(serde::ser::Error::custom)? {
            seq.serialize_element(&entry)?;
        }

        seq.end()
    }
}

impl<V, S> Extend<V> for Vector<V, S>
where
    V: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    S: StorageAdaptor,
{
    fn extend<I: IntoIterator<Item = V>>(&mut self, iter: I) {
        let iter = iter.into_iter().map(|v| (None, v));

        self.inner.extend(iter);
    }
}

impl<V, S> FromIterator<V> for Vector<V, S>
where
    V: BorshSerialize + BorshDeserialize + AsRef<[u8]>,
    S: StorageAdaptor,
{
    fn from_iter<I: IntoIterator<Item = V>>(iter: I) -> Self {
        let mut map = Vector::new_internal();

        map.extend(iter);

        map
    }
}

#[cfg(test)]
mod tests {
    use crate::collections::{Root, Vector};

    #[test]
    fn test_vector_push() {
        let mut vector = Root::new(|| Vector::new());

        let value = "test_data".to_owned();
        let result = vector.push(value.clone());
        assert!(result.is_ok());
        assert_eq!(vector.len().unwrap(), 1);
    }

    #[test]
    fn test_vector_get() {
        let mut vector = Root::new(|| Vector::new());

        let value = "test_data".to_owned();
        let _ = vector.push(value.clone()).unwrap();
        let retrieved_value = vector.get(0).unwrap();
        assert_eq!(retrieved_value, Some(value));
    }

    #[test]
    fn test_vector_update() {
        let mut vector = Root::new(|| Vector::new());

        let value1 = "test_data1".to_owned();
        let value2 = "test_data2".to_owned();
        let _ = vector.push(value1.clone()).unwrap();
        let old = vector.update(0, value2.clone()).unwrap();
        let retrieved_value = vector.get(0).unwrap();
        assert_eq!(retrieved_value, Some(value2));
        assert_eq!(old, Some(value1));
    }

    #[test]
    fn test_vector_get_non_existent() {
        let vector = Root::new(|| Vector::<String>::new());

        match vector.get(0) {
            Ok(retrieved_value) => assert_eq!(retrieved_value, None),
            Err(e) => panic!("Error occurred: {:?}", e),
        }
    }

    #[test]
    fn test_vector_pop() {
        let mut vector = Root::new(|| Vector::new());

        let value = "test_data".to_owned();
        let _ = vector.push(value.clone()).unwrap();
        let popped_value = vector.pop().unwrap();
        assert_eq!(popped_value, Some(value));
        assert_eq!(vector.len().unwrap(), 0);
    }

    #[test]
    fn test_vector_items() {
        let mut vector = Root::new(|| Vector::new());

        let value1 = "test_data1".to_owned();
        let value2 = "test_data2".to_owned();
        let _ = vector.push(value1.clone()).unwrap();
        let _ = vector.push(value2.clone()).unwrap();
        let items: Vec<String> = vector.iter().unwrap().collect();
        assert_eq!(items, vec![value1, value2]);
    }

    #[test]
    fn test_vector_contains() {
        let mut vector = Root::new(|| Vector::new());

        let value = "test_data".to_owned();
        let _ = vector.push(value.clone()).unwrap();
        assert!(vector.contains(&value).unwrap());
        let non_existent_value = "non_existent".to_owned();
        assert!(!vector.contains(&non_existent_value).unwrap());
    }

    #[test]
    fn test_vector_clear() {
        let mut vector = Root::new(|| Vector::new());

        let value = "test_data".to_owned();
        let _ = vector.push(value.clone()).unwrap();
        vector.clear().unwrap();
        assert_eq!(vector.len().unwrap(), 0);
    }

    #[test]
    fn test_vector_find() {
        let mut vector = Root::new(|| Vector::new());

        let _ = vector.push("apple".to_owned()).unwrap();
        let _ = vector.push("banana".to_owned()).unwrap();
        let _ = vector.push("cherry".to_owned()).unwrap();
        let _ = vector.push("banana".to_owned()).unwrap();

        // Find first element that starts with 'b'
        let result: Vec<String> = vector.find(|s| s.starts_with('b')).unwrap().collect();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "banana");

        // Find element that doesn't exist
        let result: Vec<String> = vector.find(|s| s.starts_with('z')).unwrap().collect();
        assert_eq!(result.len(), 0);

        // Find first element (any)
        let result: Vec<String> = vector.find(|_| true).unwrap().collect();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "apple");
    }

    #[test]
    fn test_vector_filter() {
        let mut vector = Root::new(|| Vector::new());

        let _ = vector.push("apple".to_owned()).unwrap();
        let _ = vector.push("banana".to_owned()).unwrap();
        let _ = vector.push("cherry".to_owned()).unwrap();
        let _ = vector.push("banana".to_owned()).unwrap();
        let _ = vector.push("apricot".to_owned()).unwrap();

        // Filter all elements that start with 'a'
        let result: Vec<String> = vector.filter(|s| s.starts_with('a')).unwrap().collect();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "apple");
        assert_eq!(result[1], "apricot");

        // Filter all 'banana' elements
        let result: Vec<String> = vector.filter(|s| s == "banana").unwrap().collect();
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|s| s == "banana"));

        // Filter elements that don't exist
        let result: Vec<String> = vector.filter(|s| s.starts_with('z')).unwrap().collect();
        assert_eq!(result.len(), 0);

        // Filter all elements
        let result: Vec<String> = vector.filter(|_| true).unwrap().collect();
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn test_vector_find_with_numbers() {
        let mut vector = Root::new(|| Vector::new());

        let _ = vector.push(1u32).unwrap();
        let _ = vector.push(5u32).unwrap();
        let _ = vector.push(10u32).unwrap();
        let _ = vector.push(15u32).unwrap();

        // Find first number > 7
        let result: Vec<u32> = vector.find(|&n| n > 7).unwrap().collect();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], 10);
    }

    #[test]
    fn test_vector_filter_with_numbers() {
        let mut vector = Root::new(|| Vector::new());

        let _ = vector.push(1u32).unwrap();
        let _ = vector.push(5u32).unwrap();
        let _ = vector.push(10u32).unwrap();
        let _ = vector.push(15u32).unwrap();
        let _ = vector.push(20u32).unwrap();

        // Filter all numbers > 7
        let result: Vec<u32> = vector.filter(|&n| n > 7).unwrap().collect();
        assert_eq!(result.len(), 3);
        assert_eq!(result, vec![10, 15, 20]);

        // Filter even numbers
        let result: Vec<u32> = vector.filter(|&n| n % 2 == 0).unwrap().collect();
        assert_eq!(result.len(), 2);
        assert_eq!(result, vec![10, 20]);
    }
}
