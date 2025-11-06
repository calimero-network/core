use calimero_primitives::application::ApplicationId;
use eyre::bail;
use rand::Rng;

#[allow(dead_code, reason = "utility function for application validation")]
pub fn validate_application_id(ours: &ApplicationId, theirs: &ApplicationId) -> eyre::Result<()> {
    if ours != theirs {
        bail!("application mismatch: expected {}, got {}", ours, theirs);
    }
    Ok(())
}

#[must_use]
pub fn generate_nonce() -> calimero_crypto::Nonce {
    rand::thread_rng().gen()
}
