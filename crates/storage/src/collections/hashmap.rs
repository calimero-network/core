use std::borrow::Borrow;
use std::marker::PhantomData;

use borsh::{BorshDeserialize, BorshSerialize};

// fixme! macro expects `calimero_storage` to be in deps
use crate as calimero_storage;
use crate::address::Path;
use crate::entities::{Data, Element};
use crate::interface::Interface;
use crate::collections::error::StoreError;

use crate::{AtomicUnit, Collection};

/// A map collection that stores key-value pairs.
#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(255)]
#[root]
pub struct HashMap<K, V> {
    /// The entries in the map.
    entries: Entries<K, V>,
    /// The storage element for the map.
    #[storage]
    storage: Element,
}

/// A collection of entries in a map.
#[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Entry<K, V>)]
struct Entries<K, V> {
    /// Helper to associate the generic types with the collection.
    _priv: PhantomData<(K, V)>,
}

/// An entry in a map.
#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(254)]
pub struct Entry<K, V> {
    /// The key for the entry.
    key: K,
    /// The value for the entry.
    value: V,
    /// The storage element for the entry.
    #[storage]
    storage: Element,
}

impl<K: BorshSerialize + BorshDeserialize, V: BorshSerialize + BorshDeserialize> HashMap<K, V> {
    /// Create a new map collection.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn new(path: &Path) -> Result<Self, StoreError> {
        let mut this = Self {
            entries: Entries::default(),
            storage: Element::new(path),
        };

        let _ = Interface::save(&mut this)?;

        Ok(this)
    }

    /// Insert a key-value pair into the map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn insert(&mut self, key: K, value: V) -> Result<(), StoreError> {
        let path = self.path();
        // fixme! Reusing the HashMap's path for now. We "could" concatenate, but it's
        // fixme! non-trivial and currently non-functional, so it's been left out


        let storage = Element::new(&path);
        // fixme! This uses a random id for the map's entries, which will impair
        // fixme! perf on the lookup, as we'd have to fetch and look through all
        // fixme! entries to find the one that matches the key we're looking for
        // fixme! ideally, the Id should be defined as hash(concat(map_id, key))
        // fixme! which will save on map-wide lookups, getting the item directly

        let _ = Interface::add_child_to(
            self.storage.id(),
            &mut self.entries,
            &mut Entry {
                key,
                value,
                storage,
            },
        )?;

        Ok(())
    }

    /// Get an iterator over the entries in the map.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn entries(&self) -> Result<impl Iterator<Item = (K, V)>, StoreError> {
        let entries = Interface::children_of(self.id(), &self.entries)?;

        Ok(entries.into_iter().map(|entry| (entry.key, entry.value)))
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
        Ok(Interface::child_info_for(self.id(), &self.entries)?.len())
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
        Q: PartialEq + ?Sized,
    {
        for (key_, value) in self.entries()? {
            if key_.borrow() == key {
                return Ok(Some(value));
            }
        }

        Ok(None)
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
        Q: PartialEq + ?Sized,
    {
        let entries = Interface::children_of(self.id(), &self.entries)?;

        let entry = entries.into_iter().find(|entry| entry.key.borrow() == key);

        if let Some(entry) = &entry {
            let _ = Interface::remove_child_from(self.id(), &mut self.entries, entry.id())?;
        }

        Ok(entry.map(|entry| entry.value))
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
        let entries = Interface::children_of(self.id(), &self.entries)?;

        for entry in entries {
            let _ = Interface::remove_child_from(self.id(), &mut self.entries, entry.id())?;
        }

        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(test)]
mod tests {
    use crate::address::Path;
    use crate::collections::hashmap::HashMap;

    #[test]
    fn test() {
        let mut map = HashMap::<String, String>::new(&Path::new("::map").unwrap()).expect(
            "failed to create map",
        );

        map.insert("key".to_string(), "value".to_string()).expect("insert failed");

        assert_eq!(map.get("key").expect("get failed"), Some("value".to_string()));
        assert_ne!(map.get("key").expect("get failed"), Some("value2".to_string()));

        map.insert("key".to_string(), "value2".to_string()).expect("insert failed");
        map.insert("key2".to_string(), "value".to_string()).expect("insert failed");

        assert_eq!(map.get("key").expect("get failed"), Some("value2".to_string()));
        assert_eq!(map.get("key2").expect("get failed"), Some("value".to_string()));
    }
}