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

    /// The application's `init` did not complete, so no context was created.
    ///
    /// Carries the guest's own message, which is the only thing that says what
    /// was actually wrong — almost always that the `initializationParams` do
    /// not match `init`'s signature. `#[app::init]` cannot return a `Result`
    /// (the macro rejects it), so the only way it fails is a guest panic, and
    /// the SDK's panic hook routes every one of those through `panic_utf8`;
    /// the message is therefore never empty.
    ///
    /// Typed for the same reason as [`Self::NotAGroupMember`]: it is a caller
    /// precondition, not a server fault, and callers map it to a `400` rather
    /// than letting it fall through to a generic `500` that says nothing.
    #[error("application initialization failed: {message}")]
    InitFailed {
        /// The guest's panic message, verbatim.
        message: String,
    },

    /// This node is not a member of the group the operation targets.
    ///
    /// A legitimate client-side precondition (the node hasn't joined the
    /// group, or isn't in it), not a server fault — callers map this to a
    /// `403`, never a generic `500`.
    #[error("node is not a member of group '{group_id}'")]
    NotAGroupMember {
        /// Debug rendering of the target group id (for the message only).
        group_id: String,
    },
}
