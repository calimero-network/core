//! Content-addressable storage wrapper.
//!
//! Provides an `UnorderedMap<Hash, FrozenValue<T>>` where the key is the SHA256 hash
//! of the value `T`. Data is immutable once inserted.

use super::crdt_meta::{CrdtMeta, CrdtType, Mergeable, StorageStrategy};
use super::{StorageError, StoreError, UnorderedMap};
use crate::entities::{Data, Element, StorageType};
use crate::store::{MainStorage, StorageAdaptor};
use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use super::frozen_value::FrozenValue;

type Hash = [u8; 32];

/// A wrapper for immutable, content-addressable storage.
///
/// Under the hood, this is an `UnorderedMap<Hash, FrozenValue<Vec<u8>>>`.
/// The user interacts with type `T`, but we store its serialized bytes.
#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct FrozenStorage<T: BorshSerialize + BorshDeserialize, S: StorageAdaptor = MainStorage> {
    /// The underlying map storing immutable data.
    #[borsh(bound(serialize = "", deserialize = ""))]
    pub(crate) inner: UnorderedMap<Hash, FrozenValue<T>, S>,
    /// The storage element for this FrozenStorage instance itself.
    storage: Element,
}

impl<T> FrozenStorage<T, MainStorage>
where
    T: BorshSerialize + BorshDeserialize,
{
    /// Creates a new, empty FrozenStorage.
    pub fn new() -> Self {
        Self {
            inner: UnorderedMap::new(),
            storage: Element::new(None),
        }
    }
}

impl<T> Default for FrozenStorage<T, MainStorage>
where
    T: BorshSerialize + BorshDeserialize,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T, S> FrozenStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Inserts a value into frozen storage.
    ///
    /// The value is serialized and hashed. The resulting hash is used
    /// as the key. This operation is idempotent.
    ///
    /// Returns the `Hash` (key) of the inserted data.
    ///
    /// # Errors
    /// Returns a `StoreError` if serialization or storage fails.
    pub fn insert(&mut self, value: T) -> Result<Hash, StoreError> {
        // Serialize the value to get its content-addressable key
        let data_bytes = borsh::to_vec(&value)
            .map_err(|e| StoreError::StorageError(StorageError::SerializationError(e)))?;

        let key_hash: Hash = Sha256::digest(&data_bytes).into();

        let frozen_value_bytes = FrozenValue(value);

        // Call the new method on UnorderedMap
        let _ignored = self.inner.insert_with_storage_type(
            key_hash,
            frozen_value_bytes,
            StorageType::Frozen,
            None,
        )?;

        Ok(key_hash)
    }
}

impl<T, S> FrozenStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Gets a value from frozen storage by its hash.
    /// Returns the deserialized value `T`.
    ///
    /// # Errors
    /// Returns a `StoreError` if the storage operation fails.
    pub fn get(&self, hash: &Hash) -> Result<Option<T>, StoreError> {
        Ok(self.inner.get(hash)?.map(|frozen_value| frozen_value.0))
        //// Get the Option<FrozenValue<Vec<u8>>>
        //if let Some(frozen_value_bytes) = self.inner.get(hash)? {
        //    // `frozen_value_bytes.0` is the Vec<u8>
        //    // Deserialize the bytes back into T
        //    let value = T::try_from_slice(&frozen_value_bytes.0)
        //        .map_err(|e| StoreError::StorageError(StorageError::DeserializationError(e)))?;
        //    Ok(Some(value))
        //} else {
        //    Ok(None)
        //}
    }

    /// Checks if a hash exists in frozen storage.
    ///
    /// # Errors
    /// Returns a `StoreError` if the storage operation fails.
    pub fn contains(&self, hash: &Hash) -> Result<bool, StoreError> {
        self.inner.contains(hash)
    }
}

// Implement Data for FrozenStorage so it can be nested in #[app::state]
impl<T, S> Data for FrozenStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn collections(&self) -> BTreeMap<String, Vec<crate::entities::ChildInfo>> {
        // FrozenStorage itself does not have child collections.
        // Its `inner` map is a field, not a child collection.
        // But `UnorderedMap` returns an empty `BTreeMap`, so we can
        // forward that implementation to it.
        self.inner.collections()
    }

    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }
}

// Implement Mergeable so it correctly merges in #[app::state]
impl<T, S> Mergeable for FrozenStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize + Clone,
    S: StorageAdaptor,
{
    fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
        self.inner.merge(&other.inner)
    }
}

// CrdtMeta implementation
impl<T, S> CrdtMeta for FrozenStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn crdt_type() -> CrdtType {
        CrdtType::Custom("FrozenStorage".to_owned())
    }
    fn storage_strategy() -> StorageStrategy {
        StorageStrategy::Structured
    }
    fn can_contain_crdts() -> bool {
        true // The inner map can contain CRDTs
    }
}
