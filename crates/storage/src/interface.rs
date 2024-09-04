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

use thiserror::Error as ThisError;

use crate::address::{Id, Path};
use crate::entities::Element;

/// The primary interface for the storage system.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
#[non_exhaustive]
pub struct Interface;

impl Interface {
    /// Creates a new instance of the [`Interface`].
    #[must_use]
    pub const fn new() -> Self {
        Self {}
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
    pub fn find_by_id(&self, _id: Id) -> Result<Option<Element>, StorageError> {
        unimplemented!()
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
    /// # Parameters
    ///
    /// * `id`      - The unique identifier of the [`Element`] to save.
    /// * `element` - The [`Element`] whose data should be saved. This will be
    ///               serialised and stored in the storage system.
    ///
    /// # Errors
    ///
    /// If an error occurs when interacting with the storage system, an error
    /// will be returned.
    ///
    pub fn save(&self, _id: Id, _element: &Element) -> Result<(), StorageError> {
        unimplemented!()
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
#[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Ord, ThisError)]
#[non_exhaustive]
pub enum StorageError {
    /// TODO: An error during tree validation.
    #[error("Invalid data was found for ID: {0}")]
    InvalidDataFound(Id),

    /// TODO: An error from the Store.
    #[error("Store error: {0}")]
    StoreError(String),
}
