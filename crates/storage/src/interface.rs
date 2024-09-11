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

use borsh::{to_vec, BorshDeserialize};
use calimero_store::key::Storage as StorageKey;
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::slice::Slice;
use calimero_store::Store;
use eyre::Report;
use parking_lot::RwLock;
use thiserror::Error as ThisError;

use crate::address::{Id, Path};
use crate::entities::Element;

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

    /// Finds an [`Element`] by its unique identifier.
    ///
    /// This will always retrieve a single [`Element`], if it exists, regardless
    /// of where it may be in the hierarchy, or what state it may be in.
    ///
    /// # Parameters
    ///
    /// * `id` - The unique identifier of the [`Element`] to find.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    #[allow(clippy::significant_drop_tightening)]
    pub fn find_by_id(&self, id: Id) -> Result<Option<Element>, StorageError> {
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
                let mut element =
                    Element::try_from_slice(&slice).map_err(StorageError::DeserializationError)?;
                // TODO: This is needed for now, as the field gets stored. Later we will
                // TODO: implement a custom serialiser that will skip this field along with
                // TODO: any others that should not be stored.
                element.is_dirty = false;
                Ok(Some(element))
            }
            None => Ok(None),
        }
    }

    /// Finds one or more [`Element`]s by path in the hierarchy.
    ///
    /// This will retrieve all [`Element`]s that exist at the specified path in
    /// the hierarchy. This may be a single item, or multiple items if there are
    /// multiple [`Element`]s at the same path.
    ///
    /// # Parameters
    ///
    /// * `path` - The path to the [`Element`]s to find.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    pub fn find_by_path(&self, _path: &Path) -> Result<Vec<Element>, StorageError> {
        unimplemented!()
    }

    /// Finds the children of an [`Element`] by its unique identifier.
    ///
    /// This will retrieve all [`Element`]s that are children of the specified
    /// [`Element`]. This may be a single item, or multiple items if there are
    /// multiple children. Notably, it will return [`None`] if the [`Element`]
    /// in question does not exist.
    ///
    /// # Parameters
    ///
    /// * `id` - The unique identifier of the [`Element`] to find the children
    ///          of.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    pub fn find_children_by_id(&self, _id: Id) -> Result<Option<Vec<Element>>, StorageError> {
        unimplemented!()
    }

    /// Saves an [`Element`] to the storage system.
    ///
    /// This will save the provided [`Element`] to the storage system. If the
    /// record already exists, it will be updated with the new data. If the
    /// record does not exist, it will be created.
    ///
    /// # Update guard
    ///
    /// If the provided [`Element`] is older than the existing record, the
    /// update will be ignored, and the existing record will be kept. The
    /// Boolean return value indicates whether the record was saved or not; a
    /// value of `false` indicates that the record was not saved due to this
    /// guard check — any other reason will be due to an error, and returned as
    /// such.
    ///
    /// # Dirty flag
    ///
    /// Note, if the [`Element`] is not marked as dirty, it will not be saved,
    /// but `true` will be returned. In this case, the record is considered to
    /// be up-to-date and does not need saving, and so the save operation is
    /// effectively a no-op. If necessary, this can be checked before calling
    /// [`save()](Element::save()) by calling [`is_dirty()](Element::is_dirty()).
    ///
    /// # Parameters
    ///
    /// * `id`      - The unique identifier of the [`Element`] to save.
    /// * `element` - The [`Element`] whose data should be saved. This will be
    ///               serialised and stored in the storage system.
    ///
    /// # Errors
    ///
    /// If an error occurs when serialising data or interacting with the storage
    /// system, an error will be returned.
    ///
    pub fn save(&self, id: Id, element: &mut Element) -> Result<bool, StorageError> {
        if !element.is_dirty() {
            return Ok(true);
        }
        // It is possible that the record gets added or updated after the call to
        // this find() method, and before the put() to save the new data... however,
        // this is very unlikely under our current operating model, and so the risk
        // is considered acceptable. If this becomes a problem, we should change
        // the RwLock to a ReentrantMutex, or reimplement the get() logic here to
        // occur within the write lock. But this seems unnecessary at present.
        if let Some(existing) = self.find_by_id(id)? {
            if existing.metadata.updated_at >= element.metadata.updated_at {
                return Ok(false);
            }
        }
        // TODO: It seems fairly bizarre/unexpected that the put() method is sync
        // TODO: and not async. The reasons and intentions need checking here, in
        // TODO: case this save() method should be async and wrap the blocking call
        // TODO: with spawn_blocking(). However, this is not straightforward at
        // TODO: present because Slice uses Rc internally for some reason.
        self.store
            .write()
            .put(
                &StorageKey::new(id.into()),
                Slice::from(to_vec(element).map_err(StorageError::SerializationError)?),
            )
            .map_err(StorageError::StoreError)?;
        element.is_dirty = false;
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
    #[error("Deerialization error: {0}")]
    DeserializationError(IoError),

    /// An error occurred when handling threads or async tasks.
    #[error("Dispatch error: {0}")]
    DispatchError(String),

    /// TODO: An error during tree validation.
    #[error("Invalid data was found for ID: {0}")]
    InvalidDataFound(Id),

    /// An error occurred during serialization.
    #[error("Serialization error: {0}")]
    SerializationError(IoError),

    /// TODO: An error from the Store.
    #[error("Store error: {0}")]
    StoreError(#[from] Report),
}
