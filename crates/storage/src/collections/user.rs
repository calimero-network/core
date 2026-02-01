//! User-centric storage wrapper.
//!
//! Provides an `UnorderedMap<PublicKey, T>` where the current user can
//! only `insert` into their own key-slot (`env::executor_id()`).

use super::crdt_meta::{CrdtMeta, CrdtType, Mergeable, StorageStrategy};
use super::{StoreError, UnorderedMap};
use crate::entities::{ChildInfo, Data, Element, StorageType};
use crate::env;
use crate::store::{MainStorage, StorageAdaptor};
use borsh::{BorshDeserialize, BorshSerialize};
// TODO: possibly replace with the prelude's lighter implementation of `PublicKey` to not utilize
// the whole `calimero_primitives` dependency.
use calimero_primitives::identity::PublicKey;
use std::collections::BTreeMap;

/// A wrapper for user-owned storage, mapping PublicKeys to data.
///
/// Under the hood, this is an `UnorderedMap<PublicKey, T>`.
#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct UserStorage<T: BorshSerialize + BorshDeserialize, S: StorageAdaptor = MainStorage> {
    /// The underlying map storing user data.
    #[borsh(bound(serialize = "", deserialize = ""))]
    pub(crate) inner: UnorderedMap<PublicKey, T, S>,
    /// The storage element for this UserStorage instance itself.
    storage: Element,
}

impl<T> UserStorage<T, MainStorage>
where
    T: BorshSerialize + BorshDeserialize,
{
    /// Creates a new, empty UserStorage.
    pub fn new() -> Self {
        Self {
            inner: UnorderedMap::new(),
            storage: Element::new(None),
        }
    }
}

impl<T> Default for UserStorage<T, MainStorage>
where
    T: BorshSerialize + BorshDeserialize,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T, S> UserStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Inserts or updates the data for the current executor.
    ///
    /// This is the only way to write to user storage. It automatically
    /// sets the `StorageType::User` metadata on the data.
    ///
    /// # Errors
    /// Returns a `StoreError` if the storage operation fails.
    pub fn insert(&mut self, value: T) -> Result<Option<T>, StoreError> {
        let executor_public_key: PublicKey = env::executor_id().into();

        // Construct the StorageType. It will be signed later, on the upper levels by
        // `ContextManager`.
        let storage_type = StorageType::User {
            owner: executor_public_key,
            signature_data: None,
            //signature_data: Some(crate::entities::SignatureData {
            //    nonce: 0,
            //    signature: [0u8; 64],
            //})
        };

        // Call the new method on UnorderedMap
        self.inner
            .insert_with_storage_type(executor_public_key, value, storage_type, None)
    }
}

impl<T, S> UserStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    /// Gets the data for the current executor.
    ///
    /// # Errors
    /// Returns a `StoreError` if the storage operation fails.
    pub fn get(&self) -> Result<Option<T>, StoreError> {
        let executor_public_key: PublicKey = env::executor_id().into();
        self.inner.get(&executor_public_key)
    }

    /// Gets the data for a *specific* user's PublicKey.
    ///
    /// # Errors
    /// Returns a `StoreError` if the storage operation fails.
    pub fn get_for_user(&self, user_key: &PublicKey) -> Result<Option<T>, StoreError> {
        self.inner.get(user_key)
    }

    /// Checks if data exists for the current executor.
    ///
    /// # Errors
    /// Returns a `StoreError` if the storage operation fails.
    pub fn contains_current_user(&self) -> Result<bool, StoreError> {
        let executor_public_key: PublicKey = env::executor_id().into();
        self.inner.contains(&executor_public_key)
    }

    /// Checks if data exists for a specific user.
    ///
    /// # Errors
    /// Returns a `StoreError` if the storage operation fails.
    pub fn contains_user(&self, user_key: &PublicKey) -> Result<bool, StoreError> {
        self.inner.contains(user_key)
    }

    /// Removes the data for the current executor.
    ///
    /// # Errors
    /// Returns a `StoreError` if the storage operation fails.
    pub fn remove(&mut self) -> Result<Option<T>, StoreError> {
        let executor_public_key: PublicKey = env::executor_id().into();
        self.inner.remove(&executor_public_key)
    }
}

// Implement Data for UserStorage so it can be nested in #[app::state]
impl<T, S> Data for UserStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        // UserStorage itself does not have child collections.
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
impl<T, S> Mergeable for UserStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize + Mergeable,
    S: StorageAdaptor,
{
    fn merge(&mut self, other: &Self) -> Result<(), crate::collections::crdt_meta::MergeError> {
        self.inner.merge(&other.inner)
    }
}

// CrdtMeta implementation
impl<T, S> CrdtMeta for UserStorage<T, S>
where
    T: BorshSerialize + BorshDeserialize,
    S: StorageAdaptor,
{
    fn crdt_type() -> CrdtType {
        CrdtType::Custom {
            type_name: "UserStorage".to_owned(),
        }
    }
    fn storage_strategy() -> StorageStrategy {
        StorageStrategy::Structured
    }
    fn can_contain_crdts() -> bool {
        true // The inner map can contain CRDTs
    }
}
