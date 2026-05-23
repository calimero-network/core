//! Delta-envelope signature primitive.
//!
//! Closes the anti-impersonation gap on the delta-envelope level: a
//! current group-key holder can no longer write a delta claiming
//! another member as `author_id`. The author signs a canonical
//! payload that binds `(context_id, delta_id, author_id,
//! governance_position)`; every receive path verifies before
//! applying.
//!
//! The signature primitive is intentionally separate from per-action
//! signatures (which live in `StorageType::{User, Shared}::signature_data`
//! and verify in `Interface::apply_action`). Per-action signatures
//! attribute INDIVIDUAL writes within a delta; the envelope
//! signature binds the WHOLE delta to its author. Both are needed for
//! full coverage — per-action sigs don't catch envelope forgery
//! (a current member relabeling a foreign delta as their own), and
//! the envelope signature doesn't catch per-action forgery within a
//! Public-only delta.
//!
//! ## Payload shape
//!
//! ```ignore
//! DeltaSignaturePayload {
//!     context_id,        // pins to the context (cross-context replay)
//!     delta_id,          // hash(parents || actions); commits to the content
//!     author_id,         // claimed author
//!     governance_position, // cited cut for the membership check
//! }
//! ```
//!
//! Borsh-serialized. Signed with the author's ed25519 identity key.
//! `delta_id` is the existing content hash, so committing to it covers
//! the action bytes via the hash chain.

use borsh::BorshSerialize;
use calimero_context_config::types::GovernancePosition;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;

/// Canonical payload for the delta-envelope signature. Borsh-serialized
/// and signed by `author_id`'s ed25519 key. Only used for serialization —
/// receivers re-construct it from their own data and compare signature
/// bytes, so `BorshDeserialize` isn't needed (and wouldn't work with the
/// `&GovernancePosition` borrow anyway).
#[derive(BorshSerialize)]
pub struct DeltaSignaturePayload<'a> {
    pub context_id: ContextId,
    pub delta_id: [u8; 32],
    pub author_id: PublicKey,
    pub governance_position: Option<&'a GovernancePosition>,
}

/// Borsh-serialize the canonical payload. Used at sign time (execute
/// path) and verify time (every delta receive path).
///
/// Returns `borsh::io::Error` only if the borsh writer fails on the
/// in-memory buffer — practically infallible for these field types,
/// but the result type matches `borsh::to_vec`'s shape.
pub fn delta_signature_payload(
    context_id: ContextId,
    delta_id: [u8; 32],
    author_id: PublicKey,
    governance_position: Option<&GovernancePosition>,
) -> Result<Vec<u8>, borsh::io::Error> {
    let payload = DeltaSignaturePayload {
        context_id,
        delta_id,
        author_id,
        governance_position,
    };
    borsh::to_vec(&payload)
}
