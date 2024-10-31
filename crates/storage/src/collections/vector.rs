
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
    pub fn new(path: &Path) -> Result<Self, StoreError> {
        let mut this = Self {
            entries: Entries::default(),
            storage: Element::new(path),
        };

        let _ = Interface::save(&mut this)?;

        Ok(this)
    }

    /// Insert a key-value pair into the vector.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn insert(&mut self, value: V) -> Result<(), StoreError> {
        let path = self.path();
        // fixme! Reusing the Vector's path for now. We "could" concatenate, but it's
        // fixme! non-trivial and currently non-functional, so it's been left out

        let storage = Element::new(&path);
        // fixme! This uses a random id for the vector's entries, which will impair
        // fixme! perf on the lookup, as we'd have to fetch and look through all
        // fixme! entries to find the one that matches the key we're looking for
        // fixme! ideally, the Id should be defined as hash(concat(vector_id, key))
        // fixme! which will save on vector-wide lookups, getting the item directly

        let _ = Interface::add_child_to(
            self.storage.id(),
            &mut self.entries,
            &mut Entry {
                value,
                storage,
            },
        )?;

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
    pub fn contains<Q>(&self, value: &Q) -> Result<Option<V>, StoreError>
    where
        V: Borrow<Q>,
        Q: PartialEq + ?Sized,
    {
        /*
        for (key_, value) in self.entries()? {
            if key_.borrow() == key {
                return Ok(Some(value));
            }
        }
                key,
        */

        Ok(None)
    }

    /// Remove a key from the vector, returning the value at the key if it previously existed.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn remove<Q>(&mut self, key: &Q) -> Result<Option<V>, StoreError>
    where
        V:   Borrow<Q>,
        Q: PartialEq + ?Sized,
    {
        /*
        let entries = Interface::children_of(self.id(), &self.entries)?;

        let entry = entries.into_iter().find(|entry| entry.key.borrow() == key);

        if let Some(entry) = &entry {
            let _ = Interface::remove_child_from(self.id(), &mut self.entries, entry.id())?;
        }

        Ok(entry.vector(|entry| entry.value))
        */
        Ok(None)
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

#[cfg(not(target_arch = "wasm32"))]
#[cfg(test)]
mod tests {
    use crate::collections::vector::Vector;

}