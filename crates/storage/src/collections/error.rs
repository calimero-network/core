//! Error types for storage operations.

use thiserror::Error;

use crate::address::PathError;
use crate::interface::StorageError;

/// General error type for storage operations while interacting with complex collections.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StoreError {
    /// Error while interacting with storage.
    #[error(transparent)]
    StorageError(#[from] StorageError),
    /// Error while interacting with a path.
    #[error(transparent)]
    PathError(#[from] PathError),
}
