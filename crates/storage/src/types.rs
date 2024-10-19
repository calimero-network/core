//! High-level data structures for storage.

use std::{borrow::Borrow, marker::PhantomData};

use crate::{
    address::{Path, PathError},
    entities::{Data, Element},
    interface::{Interface, StorageError},
    AtomicUnit, Collection,
};
use borsh::{BorshDeserialize, BorshSerialize};
use thiserror::Error;

use crate as calimero_storage; // macro expects `calimero_storage` to be in deps

/// General error type for storage operations while interacting with complex collections.
#[derive(Debug, Error)]
pub enum Error {
    /// Error while interacting with storage.
    #[error(transparent)]
    StorageError(#[from] StorageError),
    /// Error while interacting with a path.
    #[error(transparent)]
    PathError(#[from] PathError),
}

/// A map collection that stores key-value pairs.
#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(255)]
#[root]
pub struct Map<K, V> {
    entries: Entries<K, V>,
    #[storage]
    storage: Element,
}

/// A collection of entries in a map.
#[derive(Collection, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[children(Entry<K, V>)]
struct Entries<K, V> {
    _priv: PhantomData<(K, V)>,
}

/// An entry in a map.
#[derive(AtomicUnit, Clone, Debug, Eq, PartialEq, PartialOrd)]
#[type_id(254)]
pub struct Entry<K, V> {
    key: K,
    value: V,
    #[storage]
    storage: Element,
}

impl<K: BorshSerialize + BorshDeserialize, V: BorshSerialize + BorshDeserialize> Map<K, V> {
    /// Create a new map collection.
    pub fn new(path: &Path) -> Result<Self, Error> {
        let mut this = Self {
            entries: Entries::default(),
            storage: Element::new(path),
        };

        let _ = Interface::save(&mut this)?;

        Ok(this)
    }

    /// Insert a key-value pair into the map.
    pub fn insert(&mut self, key: K, value: V) -> Result<(), Error> {
        let path = self.path();
        // fixme! Reusing the Map's path for now. We "could" concatenate, but it's
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
    pub fn entries(&self) -> Result<impl Iterator<Item = (K, V)>, Error> {
        let entries = Interface::children_of(self.id(), &self.entries)?;

        Ok(entries.into_iter().map(|entry| (entry.key, entry.value)))
    }

    /// Get the number of entries in the map.
    pub fn len(&self) -> Result<usize, Error> {
        Ok(Interface::child_info_for(self.id(), &self.entries)?.len())
    }

    /// Get the value for a key in the map.
    pub fn get<Q>(&self, key: &Q) -> Result<Option<V>, Error>
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
    pub fn remove<Q>(&mut self, key: &Q) -> Result<Option<V>, Error>
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
    pub fn clear(&mut self) -> Result<(), Error> {
        let entries = Interface::children_of(self.id(), &self.entries)?;

        for entry in entries {
            let _ = Interface::remove_child_from(self.id(), &mut self.entries, entry.id())?;
        }

        Ok(())
    }
}
