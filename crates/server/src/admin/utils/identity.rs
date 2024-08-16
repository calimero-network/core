use calimero_primitives::identity::{KeyPair, PublicKey};
use ed25519_dalek::{SigningKey, VerifyingKey};

pub fn generate_identity_keypair() -> KeyPair {
    let member_seed = [0u8; 32];
    let member_signing_key = SigningKey::from_bytes(&member_seed);
    let member_verifying_key = VerifyingKey::from(&member_signing_key);
    KeyPair {
        public_key: PublicKey(*member_verifying_key.as_bytes()),
        private_key: Some(*member_signing_key.as_bytes()),
    }
}
