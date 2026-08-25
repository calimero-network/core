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
    /// The node cannot yet decide whether the op's signer was authorized,
    /// because the causal cut it must judge against cites history this node has
    /// not folded — a missing ancestor, or one encrypted under a key it does not
    /// hold yet.
    ///
    /// **Not a refusal, and the distinction is the whole point.** The apply path
    /// burns nothing on this outcome — the DAG head does not advance and the
    /// nonce is not consumed — so the identical call succeeds once sync or a key
    /// pull delivers what is missing. Typed for the same reason as
    /// [`Self::NotAGroupMember`], but mapping to a RETRYABLE status: a caller that
    /// sees a generic `500` cannot tell "wait, and this will work" from "no, and
    /// it never will", and those want opposite client behaviour.
    #[error(
        "authority for '{group_id}' cannot be resolved at this op's causal cut yet          — this node is missing history or a key it is entitled to; retry once it          has synced"
    )]
    AuthorityNotYetResolvable {
        /// The group whose authority could not be resolved.
        group_id: String,
    },

    #[error("node is not a member of group '{group_id}'")]
    NotAGroupMember {
        /// Debug rendering of the target group id (for the message only).
        group_id: String,
    },

    /// The key material offered for pairing carries no valid signature from the
    /// device that minted it.
    ///
    /// Typed for the same reason as [`Self::InitFailed`]: it is the caller's
    /// payload that is wrong, so it maps to a `400` rather than a generic `500`
    /// that reads as "the node broke".
    #[error(
        "refusing to certify device {device}: {cause}. The key material does not \
         come with a valid signature from the device that minted it — re-run \
         `account pair-init` and carry its statement across unaltered"
    )]
    PairingStatementInvalid {
        /// The device being certified (for the message only).
        device: String,
        /// The verification failure, verbatim.
        cause: String,
    },

    /// The confirmation code does not describe the key material that arrived.
    ///
    /// Never carries the expected code: an attacker able to drive the endpoint
    /// would otherwise learn the one value it cannot produce.
    #[error(
        "refusing to certify device {device}: the confirmation code does not \
         match the key material in this request. Either it was mistyped, or \
         the payload was altered between `account pair-init` and here — in \
         which case do not retry with the code this side computes, get it \
         from the pairing device again"
    )]
    PairingCodeMismatch {
        /// The device being certified (for the message only).
        device: String,
    },

    /// This node takes part in none of the namespaces a pairing covers, so it
    /// holds no identity to sign the endorsement with.
    ///
    /// A precondition on this node's state rather than on the request, so it maps
    /// to a `409`: the identical call works once the node takes part there.
    #[error(
        "this node takes part in none of the namespaces this pairing covers \
         ({namespaces}); it has no identity to sign with and cannot certify a \
         device there"
    )]
    PairingNoNamespaceIdentity {
        /// Debug rendering of the namespaces the pairing is gated on.
        namespaces: String,
    },

    /// No current scope key anywhere in the namespaces a pairing is gated on.
    ///
    /// A `409` for the same reason as [`Self::PairingNoNamespaceIdentity`]: the
    /// request is fine and the node is not ready to serve it yet.
    #[error(
        "this node holds no current scope key in any of {namespaces}; pairing \
         both publishes an encrypted group op and delivers that key, so neither \
         is possible yet"
    )]
    PairingNoScopeKey {
        /// Debug rendering of the namespaces the pairing is gated on.
        namespaces: String,
    },

    /// This node's own device row belongs to another account, so its root cannot
    /// certify a second device for the account named here.
    ///
    /// The right request sent to the wrong node, which is a `403`: no retry and
    /// no state change here will make it work.
    #[error(
        "this node's device belongs to account {enrolled}, not to {account} which its \
         own root owns; a paired device cannot certify further devices — run \
         this on the node that holds the account"
    )]
    PairingNotTheAccountHolder {
        /// The account this node's device row actually names.
        enrolled: String,
        /// The account its own root owns, which the pairing would certify into.
        account: String,
    },
}
