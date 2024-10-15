//! Interface for the storage system.
//!
//! This module contains the interface for the storage system, which provides
//! the basics of loading and saving data, but presents a number of helper
//! methods and additional functionality to abstract away common operations.
//!
//! This follows the repository pattern, where the interface is the primary
//! means of interacting with the storage system, rather than the ActiveRecord
//! pattern where the model is the primary means of interaction.
//!

#[cfg(test)]
#[path = "tests/interface.rs"]
mod tests;

use std::io::Error as IoError;
use std::sync::Arc;

use borsh::to_vec;
use calimero_store::key::Storage as StorageKey;
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::slice::Slice;
use calimero_store::Store;
use eyre::Report;
use parking_lot::RwLock;
use thiserror::Error as ThisError;

use crate::address::{Id, Path};
use crate::entities::{Collection, Data};

/// The primary interface for the storage system.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Interface {
    /// The backing store to use for the storage interface.
    store: Arc<RwLock<Store>>,
}

impl Interface {
    /// Creates a new instance of the [`Interface`].
    ///
    /// # Parameters
    ///
    /// * `store` - The backing store to use for the storage interface.
    ///
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self {
            store: Arc::new(RwLock::new(store)),
        }
    }

    /// Calculates the Merkle hash for the [`Element`](crate::entities::Element).
    ///
    /// This calculates the Merkle hash for the
    /// [`Element`](crate::entities::Element), which is a cryptographic hash of
    /// the significant data in the "scope" of the [`Element`](crate::entities::Element),
    /// and is used to determine whether the data has changed and is valid. It
    /// is calculated by hashing the substantive data in the [`Element`](crate::entities::Element),
    /// along with the hashes of the children of the [`Element`](crate::entities::Element),
    /// thereby representing the state of the entire hierarchy below the
    /// [`Element`](crate::entities::Element).
    ///
    /// This method is called automatically when the [`Element`](crate::entities::Element)
    /// is updated, but can also be called manually if required.
    ///
    /// # Significant data
    ///
    /// The data considered "significant" to the state of the [`Element`](crate::entities::Element),
    /// and any change to which is considered to constitute a change in the
    /// state of the [`Element`](crate::entities::Element), is:
    ///
    ///   - The ID of the [`Element`](crate::entities::Element). This should
    ///     never change. Arguably, this could be omitted, but at present it
    ///     means that empty elements are given meaningful hashes.
    ///   - The primary [`Data`] of the [`Element`](crate::entities::Element).
    ///     This is the data that the consumer application has stored in the
    ///     [`Element`](crate::entities::Element), and is the focus of the
    ///     [`Element`](crate::entities::Element).
    ///   - The metadata of the [`Element`](crate::entities::Element). This is
    ///     the system-managed properties that are used to process the
    ///     [`Element`](crate::entities::Element), but are not part of the
    ///     primary data. Arguably the Merkle hash could be considered part of
    ///     the metadata, but it is not included in the [`Data`] struct at
    ///     present (as it obviously should not contribute to the hash, i.e.
    ///     itself).
    ///
    /// Note that private data is not considered significant, as it is not part
    /// of the shared state, and therefore does not contribute to the hash.
    ///
    /// # Parameters
    ///
    /// * `element`     - The [`Element`](crate::entities::Element) to calculate
    ///                   the Merkle hash for.
    /// * `recalculate` - Whether to recalculate or use the cached value for
    ///                   child hashes. Under normal circumstances, the cached
    ///                   value should be used, as it is more efficient. The
    ///                   option to recalculate is provided for situations when
    ///                   the entire subtree needs revalidating.
    ///
    /// # Errors
    ///
    /// If there is a problem in serialising the data, an error will be
    /// returned.
    ///
    pub fn calculate_merkle_hash_for<D: Data>(
        &self,
        entity: &D,
        recalculate: bool,
    ) -> Result<[u8; 32], StorageError> {
        entity.calculate_full_merkle_hash(self, recalculate)
    }

    /// The children of the [`Element`](crate::entities::Element).
    ///
    /// This gets the children of the [`Element`](crate::entities::Element),
    /// which are the [`Element`](crate::entities::Element)s that are directly
    /// below this [`Element`](crate::entities::Element) in the hierarchy. This
    /// is a simple method that returns the children as a list, and does not
    /// provide any filtering or ordering.
    ///
    /// Notably, there is no real concept of ordering in the storage system, as
    /// the records are not ordered in any way. They are simply stored in the
    /// hierarchy, and so the order of the children is not guaranteed. Any
    /// required ordering must be done as required upon retrieval.
    ///
    /// # Determinism
    ///
    /// TODO: Update when the `child_ids` field is replaced with an index.
    ///
    /// Depending on the source, simply looping through the children may be
    /// non-deterministic. At present we are using a [`Vec`], which is
    /// deterministic, but this is a temporary measure, and the order of
    /// children under a given path is not enforced, and therefore
    /// non-deterministic. When the `child_ids` field is replaced with an index,
    /// the order will be enforced using `created_at` timestamp and/or ID.
    ///
    /// # Performance
    ///
    /// TODO: Update when the `child_ids` field is replaced with an index.
    ///
    /// Looping through children and combining their hashes into the parent is
    /// logically correct. However, it does necessitate loading all the children
    /// to get their hashes every time there is an update. The efficiency of
    /// this can and will be improved in future.
    ///
    /// # Parameters
    ///
    /// * `element` - The [`Element`](crate::entities::Element) to get the
    ///               children of.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, or a child
    /// [`Element`](crate::entities::Element) cannot be found, an error will be
    /// returned.
    ///
    pub fn children_of<C: Collection>(
        &self,
        collection: &C,
    ) -> Result<Vec<C::Child>, StorageError> {
        let mut children = Vec::new();
        for id in collection.child_ids() {
            children.push(self.find_by_id(*id)?.ok_or(StorageError::NotFound(*id))?);
        }
        Ok(children)
    }

    /// Finds an [`Element`](crate::entities::Element) by its unique identifier.
    ///
    /// This will always retrieve a single [`Element`](crate::entities::Element),
    /// if it exists, regardless of where it may be in the hierarchy, or what
    /// state it may be in.
    ///
    /// # Parameters
    ///
    /// * `id` - The unique identifier of the [`Element`](crate::entities::Element)
    ///          to find.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    #[expect(clippy::significant_drop_tightening, reason = "False positive")]
    pub fn find_by_id<D: Data>(&self, id: Id) -> Result<Option<D>, StorageError> {
        // TODO: It seems fairly bizarre/unexpected that the put() method is sync
        // TODO: and not async. The reasons and intentions need checking here, in
        // TODO: case this find() method should be async and wrap the blocking call
        // TODO: with spawn_blocking(). However, this is not straightforward at
        // TODO: present because Slice uses Rc internally for some reason.
        // TODO: let value = spawn_blocking(|| {
        // TODO:     self.store.read()
        // TODO:         .get(&StorageKey::new((*id).into()))
        // TODO:         .map_err(StorageError::StoreError)
        // TODO: }).await.map_err(|err| StorageError::DispatchError(err.to_string()))??;
        let store = self.store.read();
        let value = store
            .get(&StorageKey::new(id.into()))
            .map_err(StorageError::StoreError)?;

        match value {
            Some(slice) => {
                let mut entity =
                    D::try_from_slice(&slice).map_err(StorageError::DeserializationError)?;
                // TODO: This is needed for now, as the field gets stored. Later we will
                // TODO: implement a custom serialiser that will skip this field along with
                // TODO: any others that should not be stored.
                entity.element_mut().is_dirty = false;
                Ok(Some(entity))
            }
            None => Ok(None),
        }
    }

    /// Finds one or more [`Element`](crate::entities::Element)s by path in the
    /// hierarchy.
    ///
    /// This will retrieve all [`Element`](crate::entities::Element)s that exist
    /// at the specified path in the hierarchy. This may be a single item, or
    /// multiple items if there are multiple [`Element`](crate::entities::Element)s
    /// at the same path.
    ///
    /// # Parameters
    ///
    /// * `path` - The path to the [`Element`](crate::entities::Element)s to
    ///            find.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    pub fn find_by_path<D: Data>(&self, _path: &Path) -> Result<Vec<D>, StorageError> {
        unimplemented!()
    }

    /// Finds the children of an [`Element`](crate::entities::Element) by its
    /// unique identifier.
    ///
    /// This will retrieve all [`Element`](crate::entities::Element)s that are
    /// children of the specified [`Element`](crate::entities::Element). This
    /// may be a single item, or multiple items if there are multiple children.
    /// Notably, it will return [`None`] if the [`Element`](crate::entities::Element)
    /// in question does not exist.
    ///
    /// # Parameters
    ///
    /// * `id` - The unique identifier of the [`Element`](crate::entities::Element)
    ///          to find the children of.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    pub fn find_children_by_id<D: Data>(&self, _id: Id) -> Result<Option<Vec<D>>, StorageError> {
        unimplemented!()
    }

    /// Saves an [`Element`](crate::entities::Element) to the storage system.
    ///
    /// This will save the provided [`Element`](crate::entities::Element) to the
    /// storage system. If the record already exists, it will be updated with
    /// the new data. If the record does not exist, it will be created.
    ///
    /// # Update guard
    ///
    /// If the provided [`Element`](crate::entities::Element) is older than the
    /// existing record, the update will be ignored, and the existing record
    /// will be kept. The Boolean return value indicates whether the record was
    /// saved or not; a value of `false` indicates that the record was not saved
    /// due to this guard check — any other reason will be due to an error, and
    /// returned as such.
    ///
    /// # Dirty flag
    ///
    /// Note, if the [`Element`](crate::entities::Element) is not marked as
    /// dirty, it will not be saved, but `true` will be returned. In this case,
    /// the record is considered to be up-to-date and does not need saving, and
    /// so the save operation is effectively a no-op. If necessary, this can be
    /// checked before calling [`save()](crate::entities::Element::save()) by
    /// calling [`is_dirty()](crate::entities::Element::is_dirty()).
    ///
    /// # Merkle hash
    ///
    /// The Merkle hash of the [`Element`](crate::entities::Element) is
    /// calculated before saving, and stored in the [`Element`](crate::entities::Element)
    /// itself. This is used to determine whether the data of the [`Element`](crate::entities::Element)
    /// or its children has changed, and is used to validate the stored data.
    ///
    /// Note that if the [`Element`](crate::entities::Element) does not need
    /// saving, or cannot be saved, then the Merkle hash will not be updated.
    /// This way the hash only ever represents the state of the data that is
    /// actually stored.
    ///
    /// # Parameters
    ///
    /// * `id`      - The unique identifier of the [`Element`](crate::entities::Element)
    ///               to save.
    /// * `element` - The [`Element`](crate::entities::Element) whose data
    ///               should be saved. This will be serialised and stored in the
    ///               storage system.
    ///
    /// # Errors
    ///
    /// If an error occurs when serialising data or interacting with the storage
    /// system, an error will be returned.
    ///
    pub fn save<D: Data>(&self, id: Id, entity: &mut D) -> Result<bool, StorageError> {
        if !entity.element().is_dirty() {
            return Ok(true);
        }
        // It is possible that the record gets added or updated after the call to
        // this find() method, and before the put() to save the new data... however,
        // this is very unlikely under our current operating model, and so the risk
        // is considered acceptable. If this becomes a problem, we should change
        // the RwLock to a ReentrantMutex, or reimplement the get() logic here to
        // occur within the write lock. But this seems unnecessary at present.
        if let Some(mut existing) = self.find_by_id::<D>(id)? {
            if existing.element_mut().metadata.updated_at >= entity.element().metadata.updated_at {
                return Ok(false);
            }
        }
        // TODO: Need to propagate the change up the tree, i.e. trigger a
        // TODO: recalculation for the ancestors.
        entity.element_mut().merkle_hash = self.calculate_merkle_hash_for(entity, false)?;

        // TODO: It seems fairly bizarre/unexpected that the put() method is sync
        // TODO: and not async. The reasons and intentions need checking here, in
        // TODO: case this save() method should be async and wrap the blocking call
        // TODO: with spawn_blocking(). However, this is not straightforward at
        // TODO: present because Slice uses Rc internally for some reason.
        self.store
            .write()
            .put(
                &StorageKey::new(id.into()),
                Slice::from(to_vec(entity).map_err(StorageError::SerializationError)?),
            )
            .map_err(StorageError::StoreError)?;
        entity.element_mut().is_dirty = false;
        Ok(true)
    }

    /// Validates the stored state.
    ///
    /// This will validate the stored state of the storage system, i.e. the data
    /// that has been saved to the storage system, ensuring that it is correct
    /// and consistent. This is done by calculating Merkle hashes of the stored
    /// data, and comparing them to the expected hashes.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    pub fn validate(&self) -> Result<(), StorageError> {
        unimplemented!()
    }
}

/// Errors that can occur when working with the storage system.
#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum StorageError {
    /// An error occurred during serialization.
    #[error("Deserialization error: {0}")]
    DeserializationError(IoError),

    /// An error occurred when handling threads or async tasks.
    #[error("Dispatch error: {0}")]
    DispatchError(String),

    /// TODO: An error during tree validation.
    #[error("Invalid data was found for ID: {0}")]
    InvalidDataFound(Id),

    /// The requested record was not found, but in the context it was asked for,
    /// it was expected to be found and so this represents an error or some kind
    /// of inconsistency in the stored data.
    #[error("Record not found with ID: {0}")]
    NotFound(Id),

    /// An error occurred during serialization.
    #[error("Serialization error: {0}")]
    SerializationError(IoError),

    /// TODO: An error from the Store.
    #[error("Store error: {0}")]
    StoreError(#[from] Report),
}
