//! JavaScript-friendly wrapper types around Calimero storage collections.
//!
//! These wrappers provide byte-oriented APIs and automatically implement the
//! [`Data`](crate::entities::Data) trait so they can be persisted through the
//! existing storage interface while being convenient to expose via FFI.

use borsh::{BorshDeserialize, BorshSerialize};

use crate as calimero_storage;
use crate::collections::{error::StoreError, UnorderedMap};
use crate::entities::{Element, Metadata};
use crate::store::MainStorage;
use crate::{address::Id, Interface};

/// Macro support for deriving storage traits on the wrapper types.
use calimero_storage_macros::AtomicUnit;

/// A byte-oriented unordered map that integrates with Calimero storage.
///
/// The map stores both keys and values as raw byte arrays (`Vec<u8>`). When
/// combined with the [`Interface`](crate::Interface) API, this enables foreign
/// runtimes (QuickJS, etc.) to leverage the full CRDT semantics without
/// reimplementing collection logic.
#[derive(Debug, AtomicUnit, BorshSerialize, BorshDeserialize)]
pub struct JsUnorderedMap {
    map: UnorderedMap<Vec<u8>, Vec<u8>>,

    #[storage]
    storage: Element,
}

impl JsUnorderedMap {
    /// Creates a new JS map backed by the main storage backend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            map: UnorderedMap::default(),
            storage: Element::new(None),
        }
    }

    /// Returns the unique identifier of this collection.
    #[must_use]
    pub fn id(&self) -> Id {
        self.storage.id()
    }

    /// Returns metadata associated with the collection.
    #[must_use]
    pub fn metadata(&self) -> Metadata {
        *self.storage.metadata()
    }

    /// Grants immutable access to the underlying element.
    #[must_use]
    pub fn element(&self) -> &Element {
        &self.storage
    }

    /// Grants mutable access to the underlying element.
    #[must_use]
    pub fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }

    /// Inserts a key/value pair into the map.
    pub fn insert(&mut self, key: &[u8], value: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.map.insert(key.to_vec(), value.to_vec())
    }

    /// Retrieves the value for `key`, if present.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.map.get(&key.to_vec())
    }

    /// Removes the value for `key`, returning the previous value if it existed.
    pub fn remove(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.map.remove(&key.to_vec())
    }

    /// Checks whether `key` exists within the map.
    pub fn contains(&self, key: &[u8]) -> Result<bool, StoreError> {
        self.map.contains(&key.to_vec())
    }

    /// Returns the number of entries in the map.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.map.len()
    }

    /// Returns `true` if the map is empty.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }

    /// Persists the map using the provided interface.
    pub fn save(&mut self) -> Result<bool, crate::StorageError> {
        Interface::<MainStorage>::save(self)
    }

    /// Loads a map by identifier using the provided interface.
    pub fn load(id: Id) -> Result<Option<Self>, crate::StorageError> {
        Interface::<MainStorage>::find_by_id::<Self>(id)
    }
}

impl Default for JsUnorderedMap {
    fn default() -> Self {
        Self::new()
    }
}
