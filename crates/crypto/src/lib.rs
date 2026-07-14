use calimero_primitives::identity::{PrivateKey, PublicKey};
use ed25519_dalek::SigningKey;
use ring::{aead, hkdf};
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const NONCE_LEN: usize = 12;

// Domain-separation label for the HKDF that turns the raw ECDH point into the
// AEAD key. Bump the version suffix if the derivation ever changes.
const AEAD_KDF_INFO: &[u8] = b"calimero.sharedkey.aead.v2";

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

        // A small-order/torsion pk collapses the shared point into a tiny
        // subgroup, so the "secret" no longer depends on our scalar. Reject it.
        if decompressed.is_small_order() {
            return Err(SharedKeyError::InvalidPublicKey);
        }

        let signing_key = SigningKey::from_bytes(sk.as_bytes());
        // curve25519-dalek 4.x Scalar implements Zeroize, so Zeroizing<Scalar>
        // clears the private scalar bytes when it is dropped here.
        let scalar = Zeroizing::new(signing_key.to_scalar());
        // A raw curve point is not a uniform 256-bit key (NIST SP 800-56C), so
        // run the ECDH secret through HKDF-SHA256. IKM is secret, so zeroize it.
        let ikm = Zeroizing::new((*scalar * decompressed).compress().to_bytes());

        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &[]).extract(&*ikm);
        let mut key = Zeroizing::new([0u8; 32]);
        prk.expand(&[AEAD_KDF_INFO], hkdf::HKDF_SHA256)
            .and_then(|okm| okm.fill(&mut *key))
            .expect("HKDF-SHA256 with a 32-byte OKM is infallible");

        Ok(Self { key })
    }

    #[must_use]
    pub fn from_sk(sk: &PrivateKey) -> Self {
        Self {
            key: Zeroizing::new(*sk.as_bytes()),
        }
    }

    /// Encrypt `payload` under this key, returning the freshly generated nonce
    /// alongside the ciphertext.
    ///
    /// The nonce is drawn from a CSPRNG internally rather than accepted from the
    /// caller: reusing a nonce under one key is catastrophic for AES-GCM, and a
    /// per-call random nonce removes that footgun entirely. The returned nonce
    /// MUST be transmitted/stored next to the ciphertext so [`decrypt`] can be
    /// given it back.
    ///
    /// [`decrypt`]: SharedKey::decrypt
    #[must_use]
    pub fn encrypt(&self, payload: Vec<u8>) -> Option<(Nonce, Vec<u8>)> {
        let nonce: Nonce = rand::random();

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

        Some((nonce, cipher_text))
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

        let (nonce, encrypted_payload) = signer_shared_key
            .encrypt(payload.to_vec())
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

        let (nonce, encrypted_token) = signer_shared_key
            .encrypt(token.to_vec())
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
        let (nonce, mut encrypted) = signer_shared_key
            .encrypt(payload.to_vec())
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
        let (nonce, mut encrypted) = signer_shared_key
            .encrypt(payload.to_vec())
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

        let (seal_nonce, encrypted) = signer_shared_key
            .encrypt(payload.to_vec())
            .ok_or_eyre("encryption failed")?;
        let mut open_nonce = seal_nonce;
        open_nonce[0] ^= 0x01;

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

    #[test]
    fn test_new_rejects_small_order_public_key() {
        // The identity point (Edwards y = 1) decompresses successfully but lies
        // in the 8-torsion subgroup. This exercises the is_small_order guard,
        // distinct from the decompress-failure path above.
        let mut small_order_bytes = [0u8; 32];
        small_order_bytes[0] = 1;

        // Confirm the bytes really decompress to a small-order point, so this
        // test can't silently regress into testing the decompress-fail path.
        let point = curve25519_dalek::edwards::CompressedEdwardsY(small_order_bytes)
            .decompress()
            .expect("identity point decompresses");
        assert!(point.is_small_order());

        let signer = PrivateKey::random(&mut thread_rng());
        let small_order_pk = PublicKey::from(small_order_bytes);

        let result = SharedKey::new(&signer, &small_order_pk);
        assert!(matches!(result, Err(SharedKeyError::InvalidPublicKey)));
    }

    #[test]
    fn test_kdf_derivation_is_deterministic_and_interoperable() -> eyre::Result<()> {
        use rand::SeedableRng;

        // Fixed seed: the derivation must be reproducible across runs.
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xCA1E);
        let alice = PrivateKey::random(&mut rng);
        let bob = PrivateKey::random(&mut rng);

        let alice_key = SharedKey::new(&alice, &bob.public_key())?;
        let bob_key = SharedKey::new(&bob, &alice.public_key())?;
        // Re-derive alice's side independently; same inputs -> same key.
        let alice_key_again = SharedKey::new(&alice, &bob.public_key())?;

        let payload = b"kdf regression lock".to_vec();
        let (nonce, ciphertext) = alice_key
            .encrypt(payload.clone())
            .ok_or_eyre("encryption failed")?;

        // Cross-peer decrypt proves both sides derived the same HKDF key.
        assert_eq!(
            bob_key
                .decrypt(ciphertext.clone(), nonce)
                .ok_or_eyre("cross-peer decrypt failed")?,
            payload
        );
        // Independent re-derivation opens the same ciphertext: deterministic.
        assert_eq!(
            alice_key_again
                .decrypt(ciphertext, nonce)
                .ok_or_eyre("re-derived decrypt failed")?,
            payload
        );

        Ok(())
    }
}
