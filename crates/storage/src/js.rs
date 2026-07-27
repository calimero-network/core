//! JavaScript-friendly wrapper types around Calimero storage collections.
//!
//! These wrappers provide byte-oriented APIs and automatically implement the
//! [`Data`](crate::entities::Data) trait so they can be persisted through the
//! existing storage interface while being convenient to expose via FFI.

use borsh::{BorshDeserialize, BorshSerialize};

use crate as calimero_storage;
use crate::collections::{
    error::StoreError, Counter as StorageCounter, FrozenStorage, LwwRegister as StorageLwwRegister,
    ReplicatedGrowableArray, SortedMap as StorageSortedMap, SortedSet as StorageSortedSet,
    UnorderedMap, UnorderedSet, UserStorage, Vector,
};
use crate::entities::{Element, Metadata};
use crate::store::MainStorage;
use crate::{address::Id, Interface, StorageError};
use calimero_primitives::identity::PublicKey;

/// Macro support for deriving storage traits on the wrapper types.
use calimero_storage_macros::AtomicUnit;

/// Decoded `(key, value)` byte-pairs returned by JS collection iterators.
type JsByteEntries = Vec<(Vec<u8>, Vec<u8>)>;
/// Decoded `(public_key, value)` pairs returned by JS set iterators.
type JsKeyedEntries = Vec<([u8; 32], Vec<u8>)>;

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

    /// Rehydrates a map using a known identifier.
    ///
    /// This is primarily used when deserialising contract state: the wasm side
    /// only stores the map id, so when the state is reconstructed we need a
    /// `JsUnorderedMap` that shares the same identifier. Merely allocating the
    /// wrapper is not enough – the collection still has to be attached to the
    /// storage index so subsequent reads do not fail with "map not found".  
    /// Callers are expected to invoke [`save`](Self::save) after creating the
    /// wrapper (the runtime loaders already do this).
    #[must_use]
    pub fn new_with_id(id: Id) -> Self {
        Self {
            map: UnorderedMap::default(),
            storage: Element::new(Some(id)),
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
        self.storage.metadata().clone()
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
        Ok(self.map.get(key)?.map(|v| v.into_inner()))
    }

    /// Removes the value for `key`, returning the previous value if it existed.
    ///
    /// # Errors
    ///
    /// Returns any [`StoreError`] emitted by the storage layer.
    pub fn remove(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.map.remove(key)
    }

    /// Checks whether `key` exists within the map.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if the existence check fails.
    pub fn contains(&self, key: &[u8]) -> Result<bool, StoreError> {
        self.map.contains(key)
    }

    /// Returns all key/value pairs currently stored in the map.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if reading from storage fails.
    pub fn entries(&self) -> Result<JsByteEntries, StoreError> {
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
    /// Creates a new byte-oriented vector wrapper.
    #[must_use]
    pub fn new() -> Self {
        Self {
            vector: Vector::default(),
            storage: Element::new(None),
        }
    }

    /// Equivalent to [`new`](Self::new) but ensures the wrapper reports the
    /// provided identifier.  The runtime persists the freshly created instance
    /// immediately so that subsequent loads succeed even if the original vector
    /// was missing from storage.
    #[must_use]
    pub fn new_with_id(id: Id) -> Self {
        Self {
            vector: Vector::default(),
            storage: Element::new(Some(id)),
        }
    }

    /// Returns the unique identifier of this vector collection.
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

    /// Returns `true` if there are no entries.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
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
        Ok(self.vector.get(index)?.map(|v| v.into_inner()))
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
    /// Creates a new byte-oriented set wrapper.
    #[must_use]
    pub fn new() -> Self {
        Self {
            set: UnorderedSet::default(),
            storage: Element::new(None),
        }
    }

    /// Creates a set wrapper that reuses an existing identifier.  Just like the
    /// map/vector variants this is paired with an eager `save` on the runtime
    /// side to guarantee that the collection is registered in the storage
    /// index before it is accessed.
    #[must_use]
    pub fn new_with_id(id: Id) -> Self {
        Self {
            set: UnorderedSet::default(),
            storage: Element::new(Some(id)),
        }
    }

    /// Returns the unique identifier of this set collection.
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

    /// Returns `true` if there are no entries.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
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
    /// Creates a new LWW register wrapper storing optional byte values.
    #[must_use]
    pub fn new() -> Self {
        Self {
            register: StorageLwwRegister::new(None),
            storage: Element::new(None),
        }
    }

    /// Recreates a register wrapper for an existing identifier.  Used when state
    /// is deserialised and we only have the register id; the runtime will
    /// persist the newly constructed wrapper before it is accessed to avoid
    /// "register not found" errors.
    #[must_use]
    pub fn new_with_id(id: Id) -> Self {
        Self {
            register: StorageLwwRegister::new(None),
            storage: Element::new(Some(id)),
        }
    }

    /// Returns the unique identifier of this register.
    #[must_use]
    pub fn id(&self) -> Id {
        self.storage.id()
    }

    /// Updates the register value and bumps the wrapper storage metadata timestamp.
    pub fn set(&mut self, value: Option<&[u8]>) {
        self.storage.update();
        match value {
            Some(bytes) => self.register.set(Some(bytes.to_vec())),
            None => self.register.set(None),
        }
    }

    /// Returns the current register value.
    pub fn get(&self) -> Option<Vec<u8>> {
        self.register.get().clone()
    }

    /// Clears the register value (`None`) and updates metadata timestamp.
    pub fn clear(&mut self) {
        self.storage.update();
        self.register.set(None);
    }

    /// Returns the logical timestamp tracked by the underlying LWW register.
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
    /// Creates a new grow-only counter wrapper.
    #[must_use]
    pub fn new() -> Self {
        Self {
            counter: StorageCounter::new(),
            storage: Element::new(None),
        }
    }

    /// Rehydrates a counter using a known identifier.
    #[must_use]
    pub fn new_with_id(id: Id) -> Self {
        Self {
            counter: StorageCounter::new(),
            storage: Element::new(Some(id)),
        }
    }

    /// Returns the unique identifier of this counter collection.
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

/// A byte-oriented user storage wrapper that integrates with Calimero storage.
///
/// The storage maps PublicKeys (32 bytes) to raw byte arrays (`Vec<u8>`).
/// This enables JavaScript runtimes to use UserStorage with proper StorageType::User
/// metadata for security checks.
#[derive(Debug, AtomicUnit, BorshSerialize, BorshDeserialize)]
pub struct JsUserStorage {
    user_storage: UserStorage<Vec<u8>>,

    #[storage]
    storage: Element,
}

impl JsUserStorage {
    /// Creates a new JS user storage backed by the main storage backend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            user_storage: UserStorage::new(),
            storage: Element::new(None),
        }
    }

    /// Rehydrates a user storage using a known identifier.
    #[must_use]
    pub fn new_with_id(id: Id) -> Self {
        Self {
            user_storage: UserStorage::new(),
            storage: Element::new(Some(id)),
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
        self.storage.metadata().clone()
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

    /// Inserts a value for the current executor (user).
    ///
    /// # Errors
    ///
    /// Returns any [`StoreError`] surfaced by the underlying storage insertion.
    pub fn insert(&mut self, value: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.user_storage.insert(value.to_vec())
    }

    /// Retrieves the value for the current executor, if present.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] when the underlying storage read fails.
    pub fn get(&self) -> Result<Option<Vec<u8>>, StoreError> {
        self.user_storage.get()
    }

    /// Retrieves the value for a specific user's PublicKey, if present.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] when the underlying storage read fails.
    pub fn get_for_user(&self, user_key: &[u8; 32]) -> Result<Option<Vec<u8>>, StoreError> {
        let public_key: PublicKey = (*user_key).into();
        self.user_storage.get_for_user(&public_key)
    }

    /// Checks whether data exists for the current executor.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if the existence check fails.
    pub fn contains_current_user(&self) -> Result<bool, StoreError> {
        self.user_storage.contains_current_user()
    }

    /// Checks whether data exists for a specific user.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if the existence check fails.
    pub fn contains_user(&self, user_key: &[u8; 32]) -> Result<bool, StoreError> {
        let public_key: PublicKey = (*user_key).into();
        self.user_storage.contains_user(&public_key)
    }

    /// Returns all user/value pairs currently stored.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if reading from storage fails.
    pub fn entries(&self) -> Result<Vec<(PublicKey, Vec<u8>)>, StoreError> {
        let iter = self.user_storage.entries()?;
        Ok(iter.collect())
    }

    /// Removes the value for the current executor, returning the previous value if it existed.
    ///
    /// # Errors
    ///
    /// Returns any [`StoreError`] emitted by the storage layer.
    pub fn remove(&mut self) -> Result<Option<Vec<u8>>, StoreError> {
        self.user_storage.remove()
    }

    /// Returns all user/value pairs currently stored.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if reading from storage fails.
    pub fn entries_raw(&self) -> Result<JsKeyedEntries, StoreError> {
        let iter = self.entries()?;
        Ok(iter
            .into_iter()
            .map(|(public_key, value)| (*public_key.as_ref(), value))
            .collect())
    }

    /// Persists the user storage using the provided interface.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] produced by the storage interface.
    pub fn save(&mut self) -> Result<bool, StorageError> {
        Interface::<MainStorage>::save(self)
    }

    /// Loads a user storage by identifier using the provided interface.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the user storage cannot be fetched from storage.
    pub fn load(id: Id) -> Result<Option<Self>, StorageError> {
        Interface::<MainStorage>::find_by_id::<Self>(id)
    }
}

impl Default for JsUserStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// A byte-oriented frozen storage wrapper that integrates with Calimero storage.
///
/// The storage maps hashes (32 bytes) to raw byte arrays (`Vec<u8>`).
/// This enables JavaScript runtimes to use FrozenStorage with proper StorageType::Frozen
/// metadata for immutability checks.
#[derive(Debug, AtomicUnit, BorshSerialize, BorshDeserialize)]
pub struct JsFrozenStorage {
    frozen_storage: FrozenStorage<Vec<u8>>,

    #[storage]
    storage: Element,
}

impl JsFrozenStorage {
    /// Creates a new JS frozen storage backed by the main storage backend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            frozen_storage: FrozenStorage::new(),
            storage: Element::new(None),
        }
    }

    /// Rehydrates a frozen storage using a known identifier.
    #[must_use]
    pub fn new_with_id(id: Id) -> Self {
        Self {
            frozen_storage: FrozenStorage::new(),
            storage: Element::new(Some(id)),
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
        self.storage.metadata().clone()
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

    /// Inserts a value into frozen storage and returns its hash.
    ///
    /// # Errors
    ///
    /// Returns any [`StoreError`] surfaced by the underlying storage insertion.
    pub fn insert(&mut self, value: &[u8]) -> Result<[u8; 32], StoreError> {
        self.frozen_storage.insert(value.to_vec())
    }

    /// Retrieves the value for `hash`, if present.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] when the underlying storage read fails.
    pub fn get(&self, hash: &[u8; 32]) -> Result<Option<Vec<u8>>, StoreError> {
        self.frozen_storage.get(hash)
    }

    /// Checks whether `hash` exists within the frozen storage.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if the existence check fails.
    pub fn contains(&self, hash: &[u8; 32]) -> Result<bool, StoreError> {
        self.frozen_storage.contains(hash)
    }

    /// Persists the frozen storage using the provided interface.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] produced by the storage interface.
    pub fn save(&mut self) -> Result<bool, StorageError> {
        Interface::<MainStorage>::save(self)
    }

    /// Loads a frozen storage by identifier using the provided interface.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the frozen storage cannot be fetched from storage.
    pub fn load(id: Id) -> Result<Option<Self>, StorageError> {
        Interface::<MainStorage>::find_by_id::<Self>(id)
    }
}

impl Default for JsFrozenStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Positive-negative counter wrapper exposed to JavaScript.
///
/// Unlike [`JsCounter`] (a grow-only G-Counter), this wraps a PN-Counter and so
/// supports [`decrement`](Self::decrement); its [`value`](Self::value) is signed
/// (`i64`) because the count can go negative.
#[derive(Debug, AtomicUnit, BorshSerialize, BorshDeserialize)]
pub struct JsPnCounter {
    counter: StorageCounter<true>,

    #[storage]
    storage: Element,
}

impl JsPnCounter {
    /// Creates a new positive-negative counter wrapper.
    #[must_use]
    pub fn new() -> Self {
        Self {
            counter: StorageCounter::new(),
            storage: Element::new(None),
        }
    }

    /// Rehydrates a counter using a known identifier.
    #[must_use]
    pub fn new_with_id(id: Id) -> Self {
        Self {
            counter: StorageCounter::new(),
            storage: Element::new(Some(id)),
        }
    }

    /// Returns the unique identifier of this counter collection.
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

    /// Decrements the counter for the current executor.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the decrement operation fails.
    pub fn decrement(&mut self) -> Result<(), StoreError> {
        self.counter.decrement()
    }

    /// Returns the total signed counter value (`positive - negative`).
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying counter.
    pub fn value(&self) -> Result<i64, StoreError> {
        self.counter.value_signed()
    }

    /// Returns the net contribution for a specific executor
    /// (`positive - negative`).
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if the read operation fails, or if either
    /// per-executor count exceeds `i64::MAX`.
    pub fn get_executor_count(&self, executor_id: &[u8; 32]) -> Result<i64, StoreError> {
        let positive = i64::try_from(self.counter.get_positive_count(executor_id)?)
            .map_err(|_| StorageError::InvalidData("positive count exceeds i64::MAX".to_owned()))?;
        let negative = i64::try_from(self.counter.get_negative_count(executor_id)?)
            .map_err(|_| StorageError::InvalidData("negative count exceeds i64::MAX".to_owned()))?;
        Ok(positive.saturating_sub(negative))
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

impl Default for JsPnCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// Replicated Growable Array (collaborative text sequence) wrapper exposed to
/// JavaScript.
///
/// Text is exchanged as UTF-8 bytes: [`insert`](Self::insert) takes the bytes of
/// a UTF-8 string to splice in at a character index, and
/// [`get_text`](Self::get_text) returns the current document as UTF-8 bytes.
#[derive(Debug, AtomicUnit, BorshSerialize, BorshDeserialize)]
pub struct JsRga {
    rga: ReplicatedGrowableArray,

    #[storage]
    storage: Element,
}

impl JsRga {
    /// Creates a new RGA wrapper.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rga: ReplicatedGrowableArray::new(),
            storage: Element::new(None),
        }
    }

    /// Rehydrates an RGA using a known identifier.
    #[must_use]
    pub fn new_with_id(id: Id) -> Self {
        Self {
            rga: ReplicatedGrowableArray::new(),
            storage: Element::new(Some(id)),
        }
    }

    /// Returns the unique identifier of this collection.
    #[must_use]
    pub fn id(&self) -> Id {
        self.storage.id()
    }

    /// Inserts the UTF-8 `value` at character position `index`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if `value` is not valid UTF-8, if `index` is out of
    /// bounds, or if the underlying storage operation fails.
    pub fn insert(&mut self, index: usize, value: &[u8]) -> Result<(), StoreError> {
        let text = core::str::from_utf8(value).map_err(|_| {
            StorageError::InvalidData("RGA insert value must be valid UTF-8".to_owned())
        })?;
        self.rga.insert_str(index, text)
    }

    /// Deletes the character at position `index`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] if `index` is out of bounds or the storage
    /// operation fails.
    pub fn delete(&mut self, index: usize) -> Result<(), StoreError> {
        self.rga.delete(index)
    }

    /// Returns the current visible text as UTF-8 bytes.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if reading from storage fails.
    pub fn get_text(&self) -> Result<Vec<u8>, StoreError> {
        Ok(self.rga.get_text()?.into_bytes())
    }

    /// Returns the number of visible characters.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if reading from storage fails.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.rga.len()
    }

    /// Returns `true` if the document is empty.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] through [`len`](Self::len).
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }

    /// Persists the RGA to storage.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] when persistence fails.
    pub fn save(&mut self) -> Result<bool, StorageError> {
        Interface::<MainStorage>::save(self)
    }

    /// Loads an RGA instance by identifier.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError`] if the RGA cannot be retrieved.
    pub fn load(id: Id) -> Result<Option<Self>, StorageError> {
        Interface::<MainStorage>::find_by_id::<Self>(id)
    }
}

impl Default for JsRga {
    fn default() -> Self {
        Self::new()
    }
}

/// A byte-oriented ordered map that integrates with Calimero storage.
///
/// Same byte API and CRDT semantics as [`JsUnorderedMap`], but
/// [`entries`](Self::entries) yields pairs in ascending key (byte) order.
#[derive(Debug, AtomicUnit, BorshSerialize, BorshDeserialize)]
pub struct JsSortedMap {
    map: StorageSortedMap<Vec<u8>, Vec<u8>>,

    #[storage]
    storage: Element,
}

impl JsSortedMap {
    /// Creates a new JS sorted map backed by the main storage backend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            map: StorageSortedMap::new(),
            storage: Element::new(None),
        }
    }

    /// Rehydrates a sorted map using a known identifier.
    #[must_use]
    pub fn new_with_id(id: Id) -> Self {
        Self {
            map: StorageSortedMap::new(),
            storage: Element::new(Some(id)),
        }
    }

    /// Returns the unique identifier of this collection.
    #[must_use]
    pub fn id(&self) -> Id {
        self.storage.id()
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
        Ok(self.map.get(key)?.map(|value| (*value).clone()))
    }

    /// Removes the value for `key`, returning the previous value if it existed.
    ///
    /// # Errors
    ///
    /// Returns any [`StoreError`] emitted by the storage layer.
    pub fn remove(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.map.remove(key)
    }

    /// Checks whether `key` exists within the map.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if the existence check fails.
    pub fn contains(&self, key: &[u8]) -> Result<bool, StoreError> {
        self.map.contains(key)
    }

    /// Returns all key/value pairs in ascending key (byte) order.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if reading from storage fails.
    pub fn entries(&self) -> Result<JsByteEntries, StoreError> {
        Ok(self.map.entries()?.collect())
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

impl Default for JsSortedMap {
    fn default() -> Self {
        Self::new()
    }
}

/// A byte-oriented ordered set that integrates with Calimero storage.
///
/// Same byte API and CRDT semantics as [`JsUnorderedSet`], but
/// [`values`](Self::values) yields elements in ascending (byte) order.
#[derive(Debug, AtomicUnit, BorshSerialize, BorshDeserialize)]
pub struct JsSortedSet {
    set: StorageSortedSet<Vec<u8>>,

    #[storage]
    storage: Element,
}

impl JsSortedSet {
    /// Creates a new JS sorted set backed by the main storage backend.
    #[must_use]
    pub fn new() -> Self {
        Self {
            set: StorageSortedSet::new(),
            storage: Element::new(None),
        }
    }

    /// Rehydrates a sorted set using a known identifier.
    #[must_use]
    pub fn new_with_id(id: Id) -> Self {
        Self {
            set: StorageSortedSet::new(),
            storage: Element::new(Some(id)),
        }
    }

    /// Returns the unique identifier of this collection.
    #[must_use]
    pub fn id(&self) -> Id {
        self.storage.id()
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

    /// Returns the number of elements stored in the set.
    ///
    /// # Errors
    ///
    /// Returns any [`StoreError`] produced by the set implementation.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.set.len()
    }

    /// Returns `true` if there are no entries.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }

    /// Returns all values in ascending (byte) order.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] if reading the underlying storage fails.
    pub fn values(&self) -> Result<Vec<Vec<u8>>, StoreError> {
        Ok(self.set.iter()?.collect())
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

impl Default for JsSortedSet {
    fn default() -> Self {
        Self::new()
    }
}
