//! Common helper functions for sync protocols.
//!
//! **DRY Principle**: Extract repeated logic from protocol implementations.

use calimero_primitives::application::ApplicationId;
use eyre::bail;
use rand::Rng;

/// Validates that peer's application ID matches ours.
///
/// # Errors
///
/// Returns error if application IDs don't match.
#[allow(dead_code, reason = "utility function for application validation")]
pub fn validate_application_id(ours: &ApplicationId, theirs: &ApplicationId) -> eyre::Result<()> {
    if ours != theirs {
        bail!("application mismatch: expected {}, got {}", ours, theirs);
    }
    Ok(())
}

/// Generates a random nonce for message encryption.
#[must_use]
pub fn generate_nonce() -> calimero_crypto::Nonce {
    rand::thread_rng().gen()
}
