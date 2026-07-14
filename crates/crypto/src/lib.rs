use calimero_primitives::identity::{PrivateKey, PublicKey};
use ed25519_dalek::SigningKey;
use ring::aead;
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const NONCE_LEN: usize = 12;

pub type Nonce = [u8; NONCE_LEN];

/// Error type for shared key creation failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SharedKeyError {
    /// The public key bytes do not represent a valid Edwards Y coordinate.
    #[error("invalid public key: not a valid Edwards Y coordinate")]
    InvalidPublicKey,
}

// Clone is intentional: callers store SharedKey in EncryptionState (which
// derives Clone) and return it by value from trait methods. Each clone owns
// its bytes and is zeroized independently on drop via Zeroizing<_>.
#[derive(Clone)]
pub struct SharedKey {
    key: Zeroizing<[u8; 32]>,
}

// Explicit Zeroize impl so SharedKey satisfies a `Zeroize` bound and callers
// can eagerly wipe the key (e.g. before returning from a function) without
// waiting for drop. The actual byte clearing delegates to Zeroizing<_>.
// The `Zeroizing<_>` field's own Drop handles zeroization on drop; no manual
// Drop impl is needed (that would double-zeroize).
impl Zeroize for SharedKey {
    fn zeroize(&mut self) {
        self.key.zeroize();
    }
}

impl ZeroizeOnDrop for SharedKey {}

impl std::fmt::Debug for SharedKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SharedKey([redacted])")
    }
}

impl SharedKey {
    /// Creates a new shared key from a private key and a public key.
    ///
    /// # Errors
    ///
    /// Returns [`SharedKeyError::InvalidPublicKey`] if the public key bytes
    /// do not represent a valid Edwards Y coordinate.
    pub fn new(sk: &PrivateKey, pk: &PublicKey) -> Result<Self, SharedKeyError> {
        let decompressed = curve25519_dalek::edwards::CompressedEdwardsY(**pk)
            .decompress()
            .ok_or(SharedKeyError::InvalidPublicKey)?;

        let signing_key = SigningKey::from_bytes(sk.as_bytes());
        // curve25519-dalek 4.x Scalar implements Zeroize, so Zeroizing<Scalar>
        // clears the private scalar bytes when it is dropped here.
        let scalar = Zeroizing::new(signing_key.to_scalar());
        let shared = (*scalar * decompressed).compress().to_bytes();

        Ok(Self {
            key: Zeroizing::new(shared),
        })
    }

    #[must_use]
    pub fn from_sk(sk: &PrivateKey) -> Self {
        Self {
            key: Zeroizing::new(*sk.as_bytes()),
        }
    }

    #[must_use]
    pub fn encrypt(&self, payload: Vec<u8>, nonce: Nonce) -> Option<Vec<u8>> {
        let encryption_key =
            aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, &*self.key).ok()?);

        let mut cipher_text = payload;
        encryption_key
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::empty(),
                &mut cipher_text,
            )
            .ok()?;

        Some(cipher_text)
    }

    #[must_use]
    pub fn decrypt(&self, cipher_text: Vec<u8>, nonce: Nonce) -> Option<Vec<u8>> {
        let decryption_key =
            aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, &*self.key).ok()?);

        let mut payload = cipher_text;
        let decrypted_len = decryption_key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::empty(),
                &mut payload,
            )
            .ok()?
            .len();

        payload.truncate(decrypted_len);

        Some(payload)
    }
}

#[cfg(test)]
mod tests {
    use eyre::OptionExt;
    use rand::thread_rng;

    use super::*;

    #[test]
    fn test_encrypt_decrypt() -> eyre::Result<()> {
        let mut csprng = thread_rng();

        let signer = PrivateKey::random(&mut csprng);
        let verifier = PrivateKey::random(&mut csprng);

        let signer_shared_key = SharedKey::new(&signer, &verifier.public_key())?;
        let verifier_shared_key = SharedKey::new(&verifier, &signer.public_key())?;

        let payload = b"privacy is important";
        let nonce = [0u8; NONCE_LEN];

        let encrypted_payload = signer_shared_key
            .encrypt(payload.to_vec(), nonce)
            .ok_or_eyre("encryption failed")?;

        let decrypted_payload = verifier_shared_key
            .decrypt(encrypted_payload, nonce)
            .ok_or_eyre("decryption failed")?;

        assert_eq!(decrypted_payload, payload);
        assert_ne!(decrypted_payload, b"privacy is not important");

        Ok(())
    }

    #[test]
    fn test_decrypt_with_invalid_key() -> eyre::Result<()> {
        let mut csprng = thread_rng();

        let signer = PrivateKey::random(&mut csprng);
        let verifier = PrivateKey::random(&mut csprng);
        let invalid = PrivateKey::random(&mut csprng);

        let signer_shared_key = SharedKey::new(&signer, &verifier.public_key())?;
        let invalid_shared_key = SharedKey::new(&invalid, &invalid.public_key())?;

        let token = b"privacy is important";
        let nonce = [0u8; NONCE_LEN];

        let encrypted_token = signer_shared_key
            .encrypt(token.to_vec(), nonce)
            .ok_or_eyre("encryption failed")?;

        let decrypted_data = invalid_shared_key.decrypt(encrypted_token, nonce);

        assert!(decrypted_data.is_none());

        Ok(())
    }

    #[test]
    fn test_decrypt_with_tampered_tag() -> eyre::Result<()> {
        // AES-GCM appends a 16-byte authentication tag after the ciphertext.
        // Flipping a bit in that tag must make `open_in_place` reject the
        // message, so `decrypt` returns `None` rather than garbage plaintext.
        let mut csprng = thread_rng();
        let signer = PrivateKey::random(&mut csprng);
        let verifier = PrivateKey::random(&mut csprng);
        let signer_shared_key = SharedKey::new(&signer, &verifier.public_key())?;
        let verifier_shared_key = SharedKey::new(&verifier, &signer.public_key())?;

        let payload = b"privacy is important";
        let nonce = [0u8; NONCE_LEN];
        let mut encrypted = signer_shared_key
            .encrypt(payload.to_vec(), nonce)
            .ok_or_eyre("encryption failed")?;

        // The tag is the trailing bytes of the sealed buffer.
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0x01;

        assert!(
            verifier_shared_key.decrypt(encrypted, nonce).is_none(),
            "decrypt must reject a tampered authentication tag"
        );
        Ok(())
    }

    #[test]
    fn test_decrypt_with_tampered_ciphertext() -> eyre::Result<()> {
        // Mutating the ciphertext body (not the tag) must also fail
        // authentication — the tag covers the whole ciphertext.
        let mut csprng = thread_rng();
        let signer = PrivateKey::random(&mut csprng);
        let verifier = PrivateKey::random(&mut csprng);
        let signer_shared_key = SharedKey::new(&signer, &verifier.public_key())?;
        let verifier_shared_key = SharedKey::new(&verifier, &signer.public_key())?;

        let payload = b"privacy is important";
        let nonce = [0u8; NONCE_LEN];
        let mut encrypted = signer_shared_key
            .encrypt(payload.to_vec(), nonce)
            .ok_or_eyre("encryption failed")?;

        // Flip the first ciphertext byte (well before the appended tag).
        encrypted[0] ^= 0x01;

        assert!(
            verifier_shared_key.decrypt(encrypted, nonce).is_none(),
            "decrypt must reject tampered ciphertext"
        );
        Ok(())
    }

    #[test]
    fn test_decrypt_with_mismatched_nonce() -> eyre::Result<()> {
        // AES-GCM binds the nonce into tag verification. A ciphertext sealed
        // under nonce A must not open under nonce B, even with the right key.
        let mut csprng = thread_rng();
        let signer = PrivateKey::random(&mut csprng);
        let verifier = PrivateKey::random(&mut csprng);
        let signer_shared_key = SharedKey::new(&signer, &verifier.public_key())?;
        let verifier_shared_key = SharedKey::new(&verifier, &signer.public_key())?;

        let payload = b"privacy is important";
        let seal_nonce = [7u8; NONCE_LEN];
        let mut open_nonce = seal_nonce;
        open_nonce[0] ^= 0x01;

        let encrypted = signer_shared_key
            .encrypt(payload.to_vec(), seal_nonce)
            .ok_or_eyre("encryption failed")?;

        assert!(
            verifier_shared_key
                .decrypt(encrypted.clone(), open_nonce)
                .is_none(),
            "decrypt must fail when the nonce differs from the one used to seal"
        );
        // Sanity: the untampered ciphertext still opens under the correct nonce.
        assert_eq!(
            verifier_shared_key
                .decrypt(encrypted, seal_nonce)
                .ok_or_eyre("decrypt with correct nonce failed")?,
            payload
        );
        Ok(())
    }

    #[test]
    fn test_new_with_invalid_public_key() {
        let mut csprng = thread_rng();
        let signer = PrivateKey::random(&mut csprng);

        // Create an invalid public key. Not all 32-byte sequences represent valid
        // Edwards Y coordinates. We need a value where the computed x^2 has no
        // square root in the field. This specific value (2 followed by zeros)
        // is known to fail decompression on the Ed25519 curve.
        let mut invalid_pk_bytes = [0u8; 32];
        invalid_pk_bytes[0] = 2;
        let invalid_pk = PublicKey::from(invalid_pk_bytes);

        let result = SharedKey::new(&signer, &invalid_pk);
        assert!(result.is_err());
        assert!(matches!(result, Err(SharedKeyError::InvalidPublicKey)));
    }
}
