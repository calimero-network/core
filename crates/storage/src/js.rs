//! JavaScript-friendly wrapper types around Calimero storage collections.
//!
//! These wrappers provide byte-oriented APIs and automatically implement the
//! [`Data`](crate::entities::Data) trait so they can be persisted through the
//! existing storage interface while being convenient to expose via FFI.
//!
//! All nine wrapper types share identical `new`, `new_with_id`, `id`, `save`,
//! and `load` implementations via the [`JsCollection`] trait and the
//! [`js_collection_wrapper!`] macro.  Only the type-specific operations
//! (insert, get, increment, …) are hand-written.

use borsh::{BorshDeserialize, BorshSerialize};

use crate as calimero_storage;
use crate::collections::{
    error::StoreError, Counter as StorageCounter, FrozenStorage, LwwRegister as StorageLwwRegister,
    ReplicatedGrowableArray, UnorderedMap, UnorderedSet, UserStorage, Vector,
};
use crate::entities::{Element, Metadata};
use crate::store::MainStorage;
use crate::{address::Id, Interface, StorageError};
use calimero_primitives::identity::PublicKey;

use calimero_storage_macros::AtomicUnit;

// ---------------------------------------------------------------------------
// JsCollection trait
// ---------------------------------------------------------------------------

/// Shared lifecycle operations for all JS collection wrappers.
///
/// The default `save` and `load` methods handle the orphan-attach dance and
/// the "recreate on missing" pattern that every wrapper type needs.
pub trait JsCollection: crate::entities::Data + Sized {
    /// Creates a new, empty instance with a fresh id.
    fn collection_new() -> Self;

    /// Creates a new, empty instance reusing an existing id.
    fn collection_new_with_id(id: Id) -> Self;

    /// Returns the unique storage identifier.
    fn collection_id(&self) -> Id;

    /// Persists the collection, attaching it to the root index if needed.
    ///
    /// # Errors
    ///
    /// Returns a `String` if the storage write or root-index attachment fails.
    fn js_save(&mut self) -> Result<(), String> {
        match Interface::<MainStorage>::save(self) {
            Ok(_) => Ok(()),
            Err(StorageError::CannotCreateOrphan(_)) => {
                ensure_root_index()?;
                Interface::<MainStorage>::add_child_to(Id::root(), self)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    /// Loads an instance by id, recreating it if missing from storage.
    ///
    /// # Errors
    ///
    /// Returns a `String` if the underlying storage read or recreation fails.
    fn js_load(id: Id) -> Result<Self, String> {
        match Interface::<MainStorage>::find_by_id::<Self>(id) {
            Ok(Some(instance)) => {
                tracing::trace!(
                    target: "calimero_storage::js",
                    id = %id,
                    "loaded from storage"
                );
                Ok(instance)
            }
            Ok(None) => {
                tracing::warn!(
                    target: "calimero_storage::js",
                    id = %id,
                    "not found in storage, recreating"
                );
                let mut instance = Self::collection_new_with_id(id);
                instance.js_save()?;
                Ok(instance)
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Ensures the root index exists so that child collections can be attached.
fn ensure_root_index() -> Result<(), String> {
    use crate::entities::ChildInfo;
    use crate::env::time_now;
    use crate::index::Index;

    match Index::<MainStorage>::get_hashes_for(Id::root()) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => {
            let timestamp = time_now();
            let metadata = Metadata::new(timestamp, timestamp);
            Index::<MainStorage>::add_root(ChildInfo::new(Id::root(), [0; 32], metadata))
                .map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// js_collection_wrapper! macro
// ---------------------------------------------------------------------------

/// Generates a JS collection wrapper struct with the `JsCollection` trait impl.
///
/// The caller provides:
/// - `$name`: the wrapper struct name (e.g. `JsUnorderedMap`)
/// - `$inner_ty`: the inner collection type
/// - `$field`: the field name for the inner collection
/// - `$init`: expression to create a default inner value
macro_rules! js_collection_wrapper {
    (
        $(#[$attr:meta])*
        $name:ident {
            $field:ident : $inner_ty:ty = $init:expr
        }
    ) => {
        $(#[$attr])*
        #[derive(Debug, AtomicUnit, BorshSerialize, BorshDeserialize)]
        pub struct $name {
            $field: $inner_ty,
            #[storage]
            storage: Element,
        }

        impl JsCollection for $name {
            fn collection_new() -> Self {
                Self {
                    $field: $init,
                    storage: Element::new(None),
                }
            }

            fn collection_new_with_id(id: Id) -> Self {
                Self {
                    $field: $init,
                    storage: Element::new(Some(id)),
                }
            }

            fn collection_id(&self) -> Id {
                self.storage.id()
            }
        }

        impl $name {
            /// Creates a new, empty instance with a fresh id.
            #[must_use]
            pub fn new() -> Self {
                <Self as JsCollection>::collection_new()
            }

            /// Rehydrates an instance using a known identifier.
            #[must_use]
            pub fn new_with_id(id: Id) -> Self {
                <Self as JsCollection>::collection_new_with_id(id)
            }

            /// Returns the unique identifier of this collection.
            #[must_use]
            pub fn id(&self) -> Id {
                <Self as JsCollection>::collection_id(self)
            }

            /// Persists the collection to storage.
            ///
            /// # Errors
            ///
            /// Returns [`StorageError`] if the save operation fails.
            pub fn save(&mut self) -> Result<bool, StorageError> {
                Interface::<MainStorage>::save(self)
            }

            /// Loads a collection instance by identifier.
            ///
            /// # Errors
            ///
            /// Returns [`StorageError`] if the instance cannot be fetched.
            pub fn load(id: Id) -> Result<Option<Self>, StorageError> {
                Interface::<MainStorage>::find_by_id::<Self>(id)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Wrapper type definitions
// ---------------------------------------------------------------------------

js_collection_wrapper! {
    /// A byte-oriented unordered map that integrates with Calimero storage.
    ///
    /// The map stores both keys and values as raw byte arrays (`Vec<u8>`). When
    /// combined with the [`Interface`](crate::Interface) API, this enables foreign
    /// runtimes (QuickJS, etc.) to leverage the full CRDT semantics without
    /// reimplementing collection logic.
    JsUnorderedMap {
        map: UnorderedMap<Vec<u8>, Vec<u8>> = UnorderedMap::default()
    }
}

impl JsUnorderedMap {
    #[must_use]
    pub fn metadata(&self) -> Metadata {
        self.storage.metadata().clone()
    }

    #[must_use]
    pub fn element(&self) -> &Element {
        &self.storage
    }

    #[must_use]
    pub fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }

    /// Inserts a key-value pair into the map.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn insert(&mut self, key: &[u8], value: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.map.insert(key.to_vec(), value.to_vec())
    }

    /// Gets the value for a key.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.map.get(key)
    }

    /// Removes a key from the map.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn remove(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.map.remove(key)
    }

    /// Checks if the map contains a key.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn contains(&self, key: &[u8]) -> Result<bool, StoreError> {
        self.map.contains(key)
    }

    /// Returns all key-value pairs.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn entries(&self) -> Result<Vec<(Vec<u8>, Vec<u8>)>, StoreError> {
        let iter = self.map.entries()?;
        Ok(iter.collect::<Vec<_>>())
    }

    /// Returns the number of entries in the map.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.map.len()
    }

    /// Returns whether the map is empty.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }
}

// ---------------------------------------------------------------------------

js_collection_wrapper! {
    /// Byte-oriented ordered list wrapper for exposure over JS host functions.
    JsVector {
        vector: Vector<Vec<u8>> = Vector::default()
    }
}

impl JsVector {
    /// Returns the number of elements in the vector.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.vector.len()
    }

    /// Appends a value to the end of the vector.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn push(&mut self, value: &[u8]) -> Result<(), StoreError> {
        self.vector.push(value.to_vec())
    }

    /// Gets the element at the given index.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn get(&self, index: usize) -> Result<Option<Vec<u8>>, StoreError> {
        self.vector.get(index)
    }

    /// Updates the element at the given index.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn update(&mut self, index: usize, value: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.vector.update(index, value.to_vec())
    }

    /// Removes and returns the last element.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn pop(&mut self) -> Result<Option<Vec<u8>>, StoreError> {
        self.vector.pop()
    }

    /// Removes all elements from the vector.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn clear(&mut self) -> Result<(), StoreError> {
        self.vector.clear()
    }

    /// Returns `true` if the vector is empty.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] through the underlying [`len`](Self::len) call.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }
}

// ---------------------------------------------------------------------------

js_collection_wrapper! {
    /// Byte-oriented set wrapper exposed to JavaScript environments.
    JsUnorderedSet {
        set: UnorderedSet<Vec<u8>> = UnorderedSet::default()
    }
}

impl JsUnorderedSet {
    /// Returns the number of elements in the set.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.set.len()
    }

    /// Inserts a value into the set.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn insert(&mut self, value: &[u8]) -> Result<bool, StoreError> {
        self.set.insert(value.to_vec())
    }

    /// Checks if the set contains a value.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn contains(&self, value: &[u8]) -> Result<bool, StoreError> {
        self.set.contains(value)
    }

    /// Removes a value from the set.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn remove(&mut self, value: &[u8]) -> Result<bool, StoreError> {
        self.set.remove(value)
    }

    /// Removes all elements from the set.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn clear(&mut self) -> Result<(), StoreError> {
        self.set.clear()
    }

    /// Returns all values in the set.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn values(&self) -> Result<Vec<Vec<u8>>, StoreError> {
        let iter = self.set.iter()?;
        Ok(iter.collect::<Vec<_>>())
    }

    /// Returns `true` if the set is empty.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] through the underlying [`len`](Self::len) call.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        Ok(self.len()? == 0)
    }
}

// ---------------------------------------------------------------------------

js_collection_wrapper! {
    /// Last-write-wins register wrapper for JavaScript consumers.
    JsLwwRegister {
        register: StorageLwwRegister<Option<Vec<u8>>> = StorageLwwRegister::new(None)
    }
}

impl JsLwwRegister {
    pub fn set(&mut self, value: Option<&[u8]>) {
        self.storage.update();
        match value {
            Some(bytes) => self.register.set(Some(bytes.to_vec())),
            None => self.register.set(None),
        }
    }

    pub fn get(&self) -> Option<Vec<u8>> {
        self.register.get().clone()
    }

    pub fn clear(&mut self) {
        self.storage.update();
        self.register.set(None);
    }

    pub fn timestamp(&self) -> crate::logical_clock::HybridTimestamp {
        self.register.timestamp()
    }
}

// ---------------------------------------------------------------------------

js_collection_wrapper! {
    /// Grow-only counter wrapper exposed to JavaScript.
    JsCounter {
        counter: StorageCounter<false> = StorageCounter::new()
    }
}

impl JsCounter {
    /// Increments the counter by one.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn increment(&mut self) -> Result<(), StoreError> {
        self.counter.increment()
    }

    /// Returns the current counter value.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn value(&self) -> Result<u64, StoreError> {
        self.counter.value()
    }

    /// Returns the positive count for the given executor.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn get_executor_count(&self, executor_id: &[u8; 32]) -> Result<u64, StoreError> {
        self.counter.get_positive_count(executor_id)
    }
}

// ---------------------------------------------------------------------------

js_collection_wrapper! {
    /// Positive-negative counter wrapper exposed to JavaScript.
    ///
    /// Unlike [`JsCounter`] (grow-only), this counter supports both increment and
    /// decrement operations.
    JsPnCounter {
        counter: StorageCounter<true> = StorageCounter::new()
    }
}

impl JsPnCounter {
    /// Increments the counter by one.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn increment(&mut self) -> Result<(), StoreError> {
        self.counter.increment()
    }

    /// Decrements the counter by one.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn decrement(&mut self) -> Result<(), StoreError> {
        self.counter.decrement()
    }

    /// Returns the current counter value.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn value(&self) -> Result<i64, StoreError> {
        self.counter.value()
    }

    /// Returns the positive count for the given executor.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn get_positive_count(&self, executor_id: &[u8; 32]) -> Result<u64, StoreError> {
        self.counter.get_positive_count(executor_id)
    }

    /// Returns the negative count for the given executor.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn get_negative_count(&self, executor_id: &[u8; 32]) -> Result<u64, StoreError> {
        self.counter.get_negative_count(executor_id)
    }
}

// ---------------------------------------------------------------------------

js_collection_wrapper! {
    /// Replicated Growable Array wrapper exposed to JavaScript.
    ///
    /// Wraps [`ReplicatedGrowableArray`] for byte-level text operations from the
    /// WASM host environment.
    JsRga {
        rga: ReplicatedGrowableArray = ReplicatedGrowableArray::new()
    }
}

impl JsRga {
    /// Inserts text at the given position.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn insert(&mut self, pos: usize, text: &str) -> Result<(), StoreError> {
        self.rga.insert_str(pos, text)
    }

    /// Deletes the character at the given position.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn delete(&mut self, pos: usize) -> Result<(), StoreError> {
        self.rga.delete(pos)
    }

    /// Returns the full text content.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn get_text(&self) -> Result<String, StoreError> {
        self.rga.get_text()
    }

    /// Returns the length of the text.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn len(&self) -> Result<usize, StoreError> {
        self.rga.len()
    }

    /// Returns whether the array is empty.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn is_empty(&self) -> Result<bool, StoreError> {
        self.rga.is_empty()
    }
}

// ---------------------------------------------------------------------------

js_collection_wrapper! {
    /// A byte-oriented user storage wrapper that integrates with Calimero storage.
    ///
    /// The storage maps PublicKeys (32 bytes) to raw byte arrays (`Vec<u8>`).
    JsUserStorage {
        user_storage: UserStorage<Vec<u8>> = UserStorage::new()
    }
}

impl JsUserStorage {
    #[must_use]
    pub fn metadata(&self) -> Metadata {
        self.storage.metadata().clone()
    }

    #[must_use]
    pub fn element(&self) -> &Element {
        &self.storage
    }

    #[must_use]
    pub fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }

    /// Inserts a value for the current user.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn insert(&mut self, value: &[u8]) -> Result<Option<Vec<u8>>, StoreError> {
        self.user_storage.insert(value.to_vec())
    }

    /// Gets the value for the current user.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn get(&self) -> Result<Option<Vec<u8>>, StoreError> {
        self.user_storage.get()
    }

    /// Gets the value for the given user.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn get_for_user(&self, user_key: &[u8; 32]) -> Result<Option<Vec<u8>>, StoreError> {
        let public_key: PublicKey = (*user_key).into();
        self.user_storage.get_for_user(&public_key)
    }

    /// Checks if the current user has a value.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn contains_current_user(&self) -> Result<bool, StoreError> {
        self.user_storage.contains_current_user()
    }

    /// Checks if the given user has a value.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn contains_user(&self, user_key: &[u8; 32]) -> Result<bool, StoreError> {
        let public_key: PublicKey = (*user_key).into();
        self.user_storage.contains_user(&public_key)
    }

    /// Removes the value for the current user.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn remove(&mut self) -> Result<Option<Vec<u8>>, StoreError> {
        self.user_storage.remove()
    }

    /// Returns all user-value pairs.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn entries(&self) -> Result<Vec<([u8; 32], Vec<u8>)>, StoreError> {
        let iter = self.user_storage.inner.entries()?;
        Ok(iter
            .map(|(public_key, value)| (*public_key, value))
            .collect())
    }
}

// ---------------------------------------------------------------------------

js_collection_wrapper! {
    /// A byte-oriented frozen storage wrapper that integrates with Calimero storage.
    ///
    /// The storage maps hashes (32 bytes) to raw byte arrays (`Vec<u8>`).
    JsFrozenStorage {
        frozen_storage: FrozenStorage<Vec<u8>> = FrozenStorage::new()
    }
}

impl JsFrozenStorage {
    #[must_use]
    pub fn metadata(&self) -> Metadata {
        self.storage.metadata().clone()
    }

    #[must_use]
    pub fn element(&self) -> &Element {
        &self.storage
    }

    #[must_use]
    pub fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }

    /// Inserts a value and returns its hash.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn insert(&mut self, value: &[u8]) -> Result<[u8; 32], StoreError> {
        self.frozen_storage.insert(value.to_vec())
    }

    /// Gets the value for the given hash.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn get(&self, hash: &[u8; 32]) -> Result<Option<Vec<u8>>, StoreError> {
        self.frozen_storage.get(hash)
    }

    /// Checks if the storage contains a value for the given hash.
    ///
    /// # Errors
    ///
    /// Propagates [`StoreError`] from the underlying collection.
    pub fn contains(&self, hash: &[u8; 32]) -> Result<bool, StoreError> {
        self.frozen_storage.contains(hash)
    }
}
