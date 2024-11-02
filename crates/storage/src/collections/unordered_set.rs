use core::borrow::Borrow;
use core::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

// fixme! macro expects `calimero_storage` to be in deps
use crate as calimero_storage;
use crate::address::{Id, Path};
use crate::collections::error::StoreError;
use crate::entities::{Data, Element};
use crate::interface::Interface;
use crate::{AtomicUnit, Collection};

/// A set collection that stores unqiue values once.
#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(253)]
#[root]
pub struct UnorderedSet<V> {
    /// The prefix used for the set's entries.
    id: Id,
    /// The entries in the set.
    entries: Entries<V>,
    /// The storage element for the set.
    #[storage]
    storage: Element,
}

/// A collection of entries in a set.
#[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Entry<V>)]
struct Entries<V> {
    /// Helper to associate the generic types with the collection.
    _priv: PhantomData<V>,
}

/// An entry in a set.
#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(252)]
pub struct Entry<V> {
    /// The value for the entry.
    value: V,
    /// The storage element for the entry.
    #[storage]
    storage: Element,
}

impl<V: BorshSerialize + BorshDeserialize + AsRef<[u8]> + PartialEq> UnorderedSet<V> {
    /// Create a new set collection.
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
            id: id,
            entries: Entries::default(),
            storage: Element::new(&Path::new(format!("::unused::set::{id}::path"))?, Some(id)),
        };

        let _ = Interface::save(&mut this)?;

        Ok(this)
    }

    /// Compute the ID for a value in the set.
    fn compute_id(&self, value: &[u8]) -> Id {
        let mut hasher = Sha256::new();
        hasher.update(self.id.as_bytes());
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
    pub fn insert(&mut self, value: V) -> Result<bool, StoreError> {
        let path = self.path();

        if self.contains(&value)? {
            return Ok(false);
        }

        let storage = Element::new(&path, Some(self.compute_id(value.as_ref())));
        let _ = Interface::add_child_to(
            self.storage.id(),
            &mut self.entries,
            &mut Entry { value, storage },
        )?;

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
    pub fn entries(&self) -> Result<impl Iterator<Item = V>, StoreError> {
        let entries = Interface::children_of(self.id(), &self.entries)?;

        Ok(entries.into_iter().map(|entry| entry.value))
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
        Ok(Interface::child_info_for(self.id(), &self.entries)?.len())
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
        Q: PartialEq<V> + ?Sized + AsRef<[u8]>,
    {
        let entry = Interface::find_by_id::<Entry<V>>(self.compute_id(value.as_ref()))?;
        Ok(entry.is_some())
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
        let entry = Element::new(&self.path(), Some(self.compute_id(value.as_ref())));

        Ok(Interface::remove_child_from(
            self.id(),
            &mut self.entries,
            entry.id(),
        )?)
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
        let entries = Interface::children_of(self.id(), &self.entries)?;

        for entry in entries {
            let _ = Interface::remove_child_from(self.id(), &mut self.entries, entry.id())?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::collections::UnorderedSet;

    #[test]
    fn test_unordered_set_operations() {
        let mut set = UnorderedSet::<String>::new().expect("failed to create set");

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
        let mut set = UnorderedSet::<String>::new().expect("failed to create set");

        assert!(set.insert("value1".to_string()).expect("insert failed"));
        assert!(set.insert("value2".to_string()).expect("insert failed"));
        assert!(!set.insert("value2".to_string()).expect("insert failed"));

        assert_eq!(set.len().expect("len failed"), 2);

        assert!(set.remove("value1").expect("remove failed"));

        assert_eq!(set.len().expect("len failed"), 1);
    }

    #[test]
    fn test_unordered_set_clear() {
        let mut set = UnorderedSet::<String>::new().expect("failed to create set");

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
        let mut set = UnorderedSet::<String>::new().expect("failed to create set");

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
