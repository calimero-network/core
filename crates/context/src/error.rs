//! Typed error enums for the context crate.
//!
//! This module provides structured error types that replace string-based errors,
//! making error handling more consistent and allowing programmatic matching on errors.

use calimero_primitives::context::ContextId;
use thiserror::Error;

/// Errors that can occur during context operations.
///
/// This enum provides typed variants for various error conditions that may arise
/// when performing context-related operations, replacing string-based error messages
/// with structured, matchable error types.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ContextError {
    /// The context was deleted before the operation could complete.
    #[error("context '{context_id}' was deleted before operation could complete")]
    ContextDeleted {
        /// The ID of the context that was deleted.
        context_id: ContextId,
    },

    /// A state inconsistency was detected during execution.
    ///
    /// This occurs when the context state changes but no actions were generated,
    /// which could indicate a potential state synchronization issue.
    #[error(
        "context state changed but no actions were generated, \
         discarding execution outcome to mitigate potential state inconsistency"
    )]
    StateInconsistency,

    /// An error occurred while accessing storage.
    #[error("storage error: {message}")]
    StorageError {
        /// A description of the storage error.
        message: String,
    },
}
