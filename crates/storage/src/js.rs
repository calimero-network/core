//! JavaScript-friendly wrapper types around Calimero storage collections.
//!
//! These wrappers provide byte-oriented APIs and automatically implement the
//! [`Data`](crate::entities::Data) trait so they can be persisted through the
//! existing storage interface while being convenient to expose via FFI.

use borsh::{BorshDeserialize, BorshSerialize};

use crate as calimero_storage;
use crate::collections::{
    error::StoreError, Counter as StorageCounter, LwwRegister as StorageLwwRegister, UnorderedMap,
    UnorderedSet, Vector,
};
use crate::entities::{Element, Metadata};
use crate::store::MainStorage;
use crate::{address::Id, Interface, StorageError};

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
    ///
    /// # Errors
    ///
    /// Returns any [`StoreError`] surfaced by the underlying map insertion.
    pub fn insert(&mut self, key: &[u8], value: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.map.insert(key.to_vec(), value.to_vec())
    }

    /// Retrieves the value for `key`, if present.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] when the underlying map read fails.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.map.get(&key.to_vec())
    }

    /// Removes the value for `key`, returning the previous value if it existed.
    ///
    /// # Errors
    ///
    /// Returns any [`StoreError`] emitted by the storage layer.
    pub fn remove(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.map.remove(&key.to_vec())
    }

    /// Checks whether `key` exists within the map.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if the existence check fails.
    pub fn contains(&self, key: &[u8]) -> Result<bool, StoreError> {
        self.map.contains(&key.to_vec())
    }

    /// Returns all key/value pairs currently stored in the map.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if reading from storage fails.
    pub fn entries(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>, StoreError> {
        let iter = self.map.entries()?;
        Ok(iter.collect::<Vec<_>>())
    }

    /// Returns the number of entries in the map.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the length query cannot be satisfied.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.map.len()
    }

    /// Returns `true` if the map is empty.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] through the underlying [`len`](Self::len) call.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }

    /// Persists the map using the provided interface.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] produced by the storage interface.
    pub fn save(&mut self) -> Result<bool, StorageError> {
        Interface::<MainStorage>::save(self)
    }

    /// Loads a map by identifier using the provided interface.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the map cannot be fetched from storage.
    pub fn load(id: Id) -> Result<Option<Self>, StorageError> {
        Interface::<MainStorage>::find_by_id::<Self>(id)
    }
}

impl Default for JsUnorderedMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Byte-oriented ordered list wrapper for exposure over JS host functions.
#[derive(Debug, AtomicUnit, BorshSerialize, BorshDeserialize)]
pub struct JsVector {
    vector: Vector<Vec<u8>>,

    #[storage]
    storage: Element,
}

impl JsVector {
    #[must_use]
    pub fn new() -> Self {
        Self {
            vector: Vector::new(),
            storage: Element::new(None),
        }
    }

    #[must_use]
    pub fn id(&self) -> Id {
        self.storage.id()
    }

    /// Returns the number of elements stored in the vector.
    ///
    /// # Errors
    ///
    /// Returns any [`StoreError`] emitted by the underlying vector.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.vector.len()
    }

    /// Appends a value to the end of the vector.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if the storage write fails.
    pub fn push(&mut self, value: &[u8]) -> Result<(), StoreError> {
        self.vector.push(value.to_vec())
    }

    /// Retrieves a value at `index`, if it exists.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the underlying vector read fails.
    pub fn get(&self, index: usize) -> Result<Option<Vec<u8>>, StoreError> {
        self.vector.get(index)
    }

    /// Updates the value at `index`, returning the old value if it existed.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the storage backend.
    pub fn update(&mut self, index: usize, value: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.vector.update(index, value.to_vec())
    }

    /// Removes and returns the last element.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] emitted by the vector pop operation.
    pub fn pop(&mut self) -> Result<Option<Vec<u8>>, StoreError> {
        self.vector.pop()
    }

    /// Removes every element from the vector.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if clearing the vector fails.
    pub fn clear(&mut self) -> Result<(), StoreError> {
        self.vector.clear()
    }

    /// Persists the vector to storage.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] raised by the persistence layer.
    pub fn save(&mut self) -> Result<bool, StorageError> {
        Interface::<MainStorage>::save(self)
    }

    /// Loads a vector instance by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the vector cannot be located in storage.
    pub fn load(id: Id) -> Result<Option<Self>, StorageError> {
        Interface::<MainStorage>::find_by_id::<Self>(id)
    }
}

impl Default for JsVector {
    fn default() -> Self {
        Self::new()
    }
}

/// Byte-oriented set wrapper exposed to JavaScript environments.
#[derive(Debug, AtomicUnit, BorshSerialize, BorshDeserialize)]
pub struct JsUnorderedSet {
    set: UnorderedSet<Vec<u8>>,

    #[storage]
    storage: Element,
}

impl JsUnorderedSet {
    #[must_use]
    pub fn new() -> Self {
        Self {
            set: UnorderedSet::new(),
            storage: Element::new(None),
        }
    }

    #[must_use]
    pub fn id(&self) -> Id {
        self.storage.id()
    }

    /// Returns the number of elements stored in the set.
    ///
    /// # Errors
    ///
    /// Returns any [`StoreError`] produced by the set implementation.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.set.len()
    }

    /// Inserts `value` into the set, returning whether it was newly added.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if insertion fails.
    pub fn insert(&mut self, value: &[u8]) -> Result<bool, StoreError> {
        self.set.insert(value.to_vec())
    }

    /// Checks whether `value` exists in the set.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the membership check fails.
    pub fn contains(&self, value: &[u8]) -> Result<bool, StoreError> {
        self.set.contains(value)
    }

    /// Removes `value` from the set, returning `true` if it was present.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] emitted by the removal.
    pub fn remove(&mut self, value: &[u8]) -> Result<bool, StoreError> {
        self.set.remove(value)
    }

    /// Clears all values from the set.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the clear operation fails.
    pub fn clear(&mut self) -> Result<(), StoreError> {
        self.set.clear()
    }

    /// Returns all values contained within the set.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if reading the underlying storage fails.
    pub fn values(&self) -> Result<Vec<Vec<u8>>, StoreError> {
        let iter = self.set.iter()?;
        Ok(iter.collect::<Vec<_>>())
    }

    /// Persists the set to storage.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] raised while saving.
    pub fn save(&mut self) -> Result<bool, StorageError> {
        Interface::<MainStorage>::save(self)
    }

    /// Loads a set instance by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the set cannot be fetched from storage.
    pub fn load(id: Id) -> Result<Option<Self>, StorageError> {
        Interface::<MainStorage>::find_by_id::<Self>(id)
    }
}

impl Default for JsUnorderedSet {
    fn default() -> Self {
        Self::new()
    }
}

/// Last-write-wins register wrapper for JavaScript consumers.
#[derive(Debug, AtomicUnit, BorshSerialize, BorshDeserialize)]
pub struct JsLwwRegister {
    register: StorageLwwRegister<Option<Vec<u8>>>,

    #[storage]
    storage: Element,
}

impl JsLwwRegister {
    #[must_use]
    pub fn new() -> Self {
        Self {
            register: StorageLwwRegister::new(None),
            storage: Element::new(None),
        }
    }

    #[must_use]
    pub fn id(&self) -> Id {
        self.storage.id()
    }

    pub fn set(&mut self, value: Option<&[u8]>) {
        match value {
            Some(bytes) => self.register.set(Some(bytes.to_vec())),
            None => self.register.set(None),
        }
    }

    pub fn get(&self) -> Option<Vec<u8>> {
        self.register.get().clone()
    }

    pub fn timestamp(&self) -> crate::logical_clock::HybridTimestamp {
        self.register.timestamp()
    }

    /// Persists the register to storage.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the save operation fails.
    pub fn save(&mut self) -> Result<bool, StorageError> {
        Interface::<MainStorage>::save(self)
    }

    /// Loads a register instance by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when the register cannot be fetched from storage.
    pub fn load(id: Id) -> Result<Option<Self>, StorageError> {
        Interface::<MainStorage>::find_by_id::<Self>(id)
    }
}

impl Default for JsLwwRegister {
    fn default() -> Self {
        Self::new()
    }
}

/// Grow-only counter wrapper exposed to JavaScript.
#[derive(Debug, AtomicUnit, BorshSerialize, BorshDeserialize)]
pub struct JsCounter {
    counter: StorageCounter<false>,

    #[storage]
    storage: Element,
}

impl JsCounter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            counter: StorageCounter::new(),
            storage: Element::new(None),
        }
    }

    #[must_use]
    pub fn new_with_id(id: Id) -> Self {
        Self {
            counter: StorageCounter::new(),
            storage: Element::new(Some(id)),
        }
    }

    #[must_use]
    pub fn id(&self) -> Id {
        self.storage.id()
    }

    /// Increments the counter for the current executor.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the increment operation fails.
    pub fn increment(&mut self) -> Result<(), StoreError> {
        self.counter.increment()
    }

    /// Returns the total counter value.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying counter.
    pub fn value(&self) -> Result<u64, StoreError> {
        self.counter.value()
    }

    /// Returns the contribution for a specific executor.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the read operation fails.
    pub fn get_executor_count(&self, executor_id: &[u8; 32]) -> Result<u64, StoreError> {
        self.counter.get_positive_count(executor_id)
    }

    /// Persists the counter to storage.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when persistence fails.
    pub fn save(&mut self) -> Result<bool, StorageError> {
        Interface::<MainStorage>::save(self)
    }

    /// Loads a counter instance by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the counter cannot be retrieved.
    pub fn load(id: Id) -> Result<Option<Self>, StorageError> {
        Interface::<MainStorage>::find_by_id::<Self>(id)
    }
}

impl Default for JsCounter {
    fn default() -> Self {
        Self::new()
    }
}
