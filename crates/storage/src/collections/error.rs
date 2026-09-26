//! Error types for storage operations.

use serde::Serialize;
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
    /// Arithmetic overflow occurred during size calculation.
    #[error("arithmetic overflow: {0}")]
    ArithmeticOverflow(String),
    /// A value could not be sealed, or a sealed value did not open for this
    /// run. See [`Sealed`](super::tee_secret::Sealed).
    #[error("sealed value could not be sealed or opened here")]
    SealFailed,
}

impl Serialize for StoreError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_str(self)
    }
}
