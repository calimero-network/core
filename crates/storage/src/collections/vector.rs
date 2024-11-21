//! This module provides functionality for the vector data structure.

use core::borrow::Borrow;
use core::fmt;
use std::mem;

use borsh::{BorshDeserialize, BorshSerialize};

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
    /// Create a new vector collection.
    fn new_internal() -> Self {
        Self {
            inner: Collection::new(None),
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

    /// Get an iterator over the entries in the vector.
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

    /// Get the number of entries in the vector.
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
        for entry in self.entries()? {
            if value == entry.borrow() {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Clear the vector, removing all entries.
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

impl<V> Eq for Vector<V> where V: Eq + BorshSerialize + BorshDeserialize {}

impl<V> PartialEq for Vector<V>
where
    V: PartialEq + BorshSerialize + BorshDeserialize,
{
    fn eq(&self, other: &Self) -> bool {
        self.entries().unwrap().eq(other.entries().unwrap())
    }
}

impl<V> Ord for Vector<V>
where
    V: Ord + BorshSerialize + BorshDeserialize,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.entries().unwrap().cmp(other.entries().unwrap())
    }
}

impl<V> PartialOrd for Vector<V>
where
    V: PartialOrd + BorshSerialize + BorshDeserialize,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.entries()
            .unwrap()
            .partial_cmp(other.entries().unwrap())
    }
}

impl<V> fmt::Debug for Vector<V>
where
    V: fmt::Debug + BorshSerialize + BorshDeserialize,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            f.debug_struct("Vector")
                .field("entries", &self.inner)
                .finish()
        } else {
            f.debug_list().entries(self.entries().unwrap()).finish()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::collections::{Root, Vector};

    #[test]
    fn test_vector_push() {
        let mut vector = Root::new(|| Vector::new());

        let value = "test_data".to_string();
        let result = vector.push(value.clone());
        assert!(result.is_ok());
        assert_eq!(vector.len().unwrap(), 1);
    }

    #[test]
    fn test_vector_get() {
        let mut vector = Root::new(|| Vector::new());

        let value = "test_data".to_string();
        let _ = vector.push(value.clone()).unwrap();
        let retrieved_value = vector.get(0).unwrap();
        assert_eq!(retrieved_value, Some(value));
    }

    #[test]
    fn test_vector_update() {
        let mut vector = Root::new(|| Vector::new());

        let value1 = "test_data1".to_string();
        let value2 = "test_data2".to_string();
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

        let value = "test_data".to_string();
        let _ = vector.push(value.clone()).unwrap();
        let popped_value = vector.pop().unwrap();
        assert_eq!(popped_value, Some(value));
        assert_eq!(vector.len().unwrap(), 0);
    }

    #[test]
    fn test_vector_entries() {
        let mut vector = Root::new(|| Vector::new());

        let value1 = "test_data1".to_string();
        let value2 = "test_data2".to_string();
        let _ = vector.push(value1.clone()).unwrap();
        let _ = vector.push(value2.clone()).unwrap();
        let entries: Vec<String> = vector.entries().unwrap().collect();
        assert_eq!(entries, vec![value1, value2]);
    }

    #[test]
    fn test_vector_contains() {
        let mut vector = Root::new(|| Vector::new());

        let value = "test_data".to_string();
        let _ = vector.push(value.clone()).unwrap();
        assert!(vector.contains(&value).unwrap());
        let non_existent_value = "non_existent".to_string();
        assert!(!vector.contains(&non_existent_value).unwrap());
    }

    #[test]
    fn test_vector_clear() {
        let mut vector = Root::new(|| Vector::new());

        let value = "test_data".to_string();
        let _ = vector.push(value.clone()).unwrap();
        vector.clear().unwrap();
        assert_eq!(vector.len().unwrap(), 0);
    }
}
