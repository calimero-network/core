use calimero_primitives::identity::{KeyPair, PublicKey};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::RngCore;

#[must_use]
pub fn generate_identity_keypair() -> KeyPair {
    let member_seed = [0u8; 32];
    let member_signing_key = SigningKey::from_bytes(&member_seed);
    let member_verifying_key = VerifyingKey::from(&member_signing_key);
    KeyPair {
        public_key: PublicKey(*member_verifying_key.as_bytes()),
        private_key: Some(*member_signing_key.as_bytes()),
    }
}

#[must_use]
pub fn generate_context_id() -> calimero_primitives::context::ContextId {
    // Create a Send-able RNG
    let mut rng = rand::thread_rng();
    // Generate a key pair for the context ID
    let mut context_seed = [0u8; 32];
    rng.fill_bytes(&mut context_seed);
    let context_signing_key = SigningKey::from_bytes(&context_seed);
    let context_verifying_key = VerifyingKey::from(&context_signing_key);
    calimero_primitives::context::ContextId::from(*context_verifying_key.as_bytes())
}
