use core::borrow::Borrow;
use core::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};

// fixme! macro expects `calimero_storage` to be in deps
use crate::address::{Id, Path};
use crate::collections::error::StoreError;
use crate::entities::{Data, Element};
use crate::interface::{Interface, StorageError};
use crate::{self as calimero_storage, AtomicUnit, Collection};

/// A vector collection that stores key-value pairs.
#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(251)]
#[root]
pub struct Vector<V> {
    /// The entries in the vector.
    entries: Entries<V>,
    /// The storage element for the vector.
    #[storage]
    storage: Element,
}

/// A collection of entries in a vector.
#[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Entry<V>)]
struct Entries<V> {
    /// Helper to associate the generic types with the collection.
    _priv: PhantomData<V>,
}

/// An entry in a vector.
#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(250)]
pub struct Entry<V> {
    /// The value for the entry.
    value: V,
    /// The storage element for the entry.
    #[storage]
    storage: Element,
}

impl<V: BorshSerialize + BorshDeserialize> Vector<V> {
    /// Create a new vector collection.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn new() -> Result<Self, StoreError> {
        let id = Id::random();
        let mut this = Self {
            entries: Entries::default(),
            storage: Element::new(
                &Path::new(format!("::unused::vector::{id}::path"))?,
                Some(id),
            ),
        };

        let _ = Interface::save(&mut this)?;

        Ok(this)
    }

    /// Add a value to the end of the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn push(&mut self, value: V) -> Result<bool, StoreError> {
        let mut entry = Entry {
            value,
            storage: Element::new(&self.path(), None),
        };

        Ok(Interface::add_child_to(
            self.id(),
            &mut self.entries,
            &mut entry,
        )?)
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
        let id = match Interface::child_info_for(self.id(), &self.entries)?
            .into_iter()
            .last()
        {
            Some(info) => info.id(),
            None => return Ok(None),
        };
        let entry = Interface::find_by_id::<Entry<V>>(id)?;
        let _ = Interface::remove_child_from(self.id(), &mut self.entries, id)?;
        Ok(entry.map(|e| e.value))
    }

    /// Get the raw storage entry at a specific index in the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    fn get_raw(&self, index: usize) -> Result<Option<Entry<V>>, StoreError> {
        let id = match Interface::child_info_for(self.id(), &self.entries)?.get(index) {
            Some(info) => info.id(),
            None => return Ok(None),
        };

        let entry = match Interface::find_by_id::<Entry<V>>(id) {
            Ok(entry) => entry,
            Err(_) => return Ok(None),
        };

        Ok(entry)
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
        Ok(self.get_raw(index)?.map(|entry| entry.value))
    }

    /// Update the value at a specific index in the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn update(&mut self, index: usize, value: V) -> Result<(), StoreError> {
        let mut entry = self.get_raw(index)?.ok_or(StoreError::StorageError(
            StorageError::ActionNotAllowed("error".to_owned()),
        ))?;

        // has to be called to update the entry
        entry.value = value;
        entry.element_mut().update();

        let _ = Interface::save::<Entry<V>>(&mut entry)?;

        Ok(())
    }

    /// Get an iterator over the entries in the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn entries(&self) -> Result<impl Iterator<Item = V>, StoreError> {
        let entries = Interface::children_of(self.id(), &self.entries)?;

        Ok(entries.into_iter().map(|entry| (entry.value)))
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
        Ok(Interface::child_info_for(self.id(), &self.entries)?.len())
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
        Q: PartialEq<V> + ?Sized,
    {
        for entry in self.entries()? {
            if value.borrow() == &entry {
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
        let entries = Interface::children_of(self.id(), &self.entries)?;

        for entry in entries {
            let _ = Interface::remove_child_from(self.id(), &mut self.entries, entry.id())?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::collections::error::StoreError;
    use crate::collections::vector::Vector;

    #[test]
    fn test_vector_new() {
        let vector: Result<Vector<String>, StoreError> = Vector::new();
        assert!(vector.is_ok());
    }

    #[test]
    fn test_vector_push() {
        let mut vector: Vector<String> = Vector::new().unwrap();
        let value = "test_data".to_string();
        let result = vector.push(value.clone());
        assert!(result.is_ok());
        assert_eq!(vector.len().unwrap(), 1);
    }

    #[test]
    fn test_vector_get() {
        let mut vector: Vector<String> = Vector::new().unwrap();
        let value = "test_data".to_string();
        let _ = vector.push(value.clone()).unwrap();
        let retrieved_value = vector.get(0).unwrap();
        assert_eq!(retrieved_value, Some(value));
    }

    #[test]
    fn test_vector_update() {
        let mut vector: Vector<String> = Vector::new().unwrap();
        let value1 = "test_data1".to_string();
        let value2 = "test_data2".to_string();
        let _ = vector.push(value1.clone()).unwrap();
        let _ = vector.update(0, value2.clone()).unwrap();
        let retrieved_value = vector.get(0).unwrap();
        assert_eq!(retrieved_value, Some(value2));
    }

    #[test]
    fn test_vector_get_non_existent() {
        let vector: Vector<String> = Vector::new().unwrap();
        match vector.get(0) {
            Ok(retrieved_value) => assert_eq!(retrieved_value, None),
            Err(e) => panic!("Error occurred: {:?}", e),
        }
    }

    #[test]
    fn test_vector_pop() {
        let mut vector: Vector<String> = Vector::new().unwrap();
        let value = "test_data".to_string();
        let _ = vector.push(value.clone()).unwrap();
        let popped_value = vector.pop().unwrap();
        assert_eq!(popped_value, Some(value));
        assert_eq!(vector.len().unwrap(), 0);
    }

    #[test]
    fn test_vector_entries() {
        let mut vector: Vector<String> = Vector::new().unwrap();
        let value1 = "test_data1".to_string();
        let value2 = "test_data2".to_string();
        let _ = vector.push(value1.clone()).unwrap();
        let _ = vector.push(value2.clone()).unwrap();
        let entries: Vec<String> = vector.entries().unwrap().collect();
        assert_eq!(entries, vec![value1, value2]);
    }

    #[test]
    fn test_vector_contains() {
        let mut vector: Vector<String> = Vector::new().unwrap();
        let value = "test_data".to_string();
        let _ = vector.push(value.clone()).unwrap();
        assert!(vector.contains(&value).unwrap());
        let non_existent_value = "non_existent".to_string();
        assert!(!vector.contains(&non_existent_value).unwrap());
    }

    #[test]
    fn test_vector_clear() {
        let mut vector: Vector<String> = Vector::new().unwrap();
        let value = "test_data".to_string();
        let _ = vector.push(value.clone()).unwrap();
        vector.clear().unwrap();
        assert_eq!(vector.len().unwrap(), 0);
    }
}
