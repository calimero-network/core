//! Error types for the storage system.

use std::io::Error as IoError;

use eyre::Report;
use serde::Serialize;
use thiserror::Error as ThisError;

use crate::address::Id;

/// Errors that can occur when working with the storage system.
#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum StorageError {
    /// The requested action is not allowed.
    #[error("Action not allowed: {0}")]
    ActionNotAllowed(String),

    /// An attempt was made to create an orphan, i.e. an entity that has not
    /// been registered as either a root or having a parent. This was probably
    /// cause by calling `save()` without calling `add_child_to()` first.
    #[error("Cannot create orphan with ID: {0}")]
    CannotCreateOrphan(Id),

    /// An error occurred during deserialization.
    #[error("Deserialization error: {0}")]
    DeserializationError(IoError),

    /// An index entry was not found for the specified entity. This would
    /// indicate a bug in the system.
    #[error("Index not found for ID: {0}")]
    IndexNotFound(Id),

    /// The requested record was not found, but in the context it was asked for,
    /// it was expected to be found and so this represents an error or some kind
    /// of inconsistency in the stored data.
    #[error("Record not found with ID: {0}")]
    NotFound(Id),

    /// An error occurred during serialization.
    #[error("Serialization error: {0}")]
    SerializationError(IoError),

    /// An error from the Store.
    #[error("Store error: {0}")]
    StoreError(#[from] Report),

    /// An unexpected ID was encountered.
    #[error("Unexpected ID: {0}")]
    UnexpectedId(Id),
}

impl Serialize for StorageError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match *self {
            Self::ActionNotAllowed(ref err) => serializer.serialize_str(err),
            Self::DeserializationError(ref err) | Self::SerializationError(ref err) => {
                serializer.serialize_str(&err.to_string())
            }
            Self::CannotCreateOrphan(id)
            | Self::IndexNotFound(id)
            | Self::UnexpectedId(id)
            | Self::NotFound(id) => serializer.serialize_str(&id.to_string()),
            Self::StoreError(ref err) => serializer.serialize_str(&err.to_string()),
        }
    }
}
