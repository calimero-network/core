use calimero_primitives::identity::{PrivateKey, PublicKey};
use ed25519_dalek::SigningKey;
use ring::{aead, hkdf};
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const NONCE_LEN: usize = 12;

// Domain-separation label for the HKDF that turns the raw ECDH point into the
// AEAD key. Bump the version suffix if the derivation ever changes.
const AEAD_KDF_INFO: &[u8] = b"calimero.sharedkey.aead.v2";

// Separate KDF label for X25519 agreements. Distinct from `AEAD_KDF_INFO` so a
// key derived from an Ed25519-converted agreement and one derived from a native
// X25519 agreement can never collide, even given the same raw point.
const X25519_KDF_INFO: &[u8] = b"calimero.sharedkey.x25519.aead.v1";

pub type Nonce = [u8; NONCE_LEN];

/// Error type for shared key creation failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SharedKeyError {
    /// The public key bytes do not represent a valid Edwards Y coordinate.
    #[error("invalid public key: not a valid Edwards Y coordinate")]
    InvalidPublicKey,
    /// The X25519 agreement collapsed to the identity point, which means the
    /// peer's key was low-order and the result does not depend on our scalar.
    #[error("invalid X25519 public key: agreement produced the identity point")]
    DegenerateAgreement,
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

/// An X25519 secret used **only** for key agreement.
///
/// Deliberately distinct from [`PrivateKey`], which signs. A device carries
/// both, rather than one Ed25519 key doing double duty as signer and
/// Diffie-Hellman secret — single-key dual-use across a signature scheme and a
/// DH is a known footgun, and separate types make passing one where the other
/// belongs a compile error rather than a review question.
#[derive(Clone, ZeroizeOnDrop)]
pub struct X25519SecretKey([u8; 32]);

impl std::fmt::Debug for X25519SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "X25519SecretKey([redacted])")
    }
}

impl From<[u8; 32]> for X25519SecretKey {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

impl X25519SecretKey {
    /// Generate a fresh agreement secret.
    pub fn random<R: rand::CryptoRng + rand::RngCore>(csprng: &mut R) -> Self {
        let mut bytes = [0u8; 32];
        csprng.fill_bytes(&mut bytes);
        let key = Self(bytes);
        // The local copy is moved into the key above; wipe the stack copy so it
        // does not outlive the move.
        bytes.zeroize();
        key
    }

    /// The matching public key.
    ///
    /// Clamping happens inside `mul_clamped`, so the stored bytes stay the raw
    /// secret and every use clamps identically — rather than clamping once at
    /// construction and hoping every later path agrees.
    #[must_use]
    pub fn public_key(&self) -> X25519PublicKey {
        X25519PublicKey(
            curve25519_dalek::constants::X25519_BASEPOINT
                .mul_clamped(self.0)
                .to_bytes(),
        )
    }

    /// The raw secret bytes, for the few storage layers that must persist them.
    ///
    /// # Security
    /// Same contract as [`PrivateKey::as_bytes`]: never log or copy beyond a
    /// tightly scoped cryptographic use — copies are not covered by the
    /// zeroize-on-drop guarantee.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// An X25519 public key — the recipient of a wrapped scope key.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct X25519PublicKey([u8; 32]);

impl From<[u8; 32]> for X25519PublicKey {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

impl X25519PublicKey {
    /// The raw 32 bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
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

    /// Derive a shared key from a native X25519 agreement.
    ///
    /// Used for scope-key delivery, where the recipient is a *device* and the
    /// key it receives on is not the key it signs with.
    ///
    /// # Errors
    /// [`SharedKeyError::DegenerateAgreement`] when the peer's key is
    /// low-order. Such a key forces the agreement into a tiny subgroup, so the
    /// result stops depending on our scalar and the "secret" becomes
    /// predictable — the X25519 analogue of the small-order check
    /// [`SharedKey::new`] performs on the Edwards side.
    pub fn from_x25519(sk: &X25519SecretKey, pk: &X25519PublicKey) -> Result<Self, SharedKeyError> {
        let shared = curve25519_dalek::montgomery::MontgomeryPoint(*pk.as_bytes())
            .mul_clamped(*sk.as_bytes());
        if shared.to_bytes() == [0u8; 32] {
            return Err(SharedKeyError::DegenerateAgreement);
        }

        // A raw curve point is not a uniform 256-bit key (NIST SP 800-56C), so
        // run it through HKDF-SHA256 exactly as the Edwards path does.
        let ikm = Zeroizing::new(shared.to_bytes());
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &[]).extract(&*ikm);
        let mut key = Zeroizing::new([0u8; 32]);
        prk.expand(&[X25519_KDF_INFO], hkdf::HKDF_SHA256)
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

    /// Generates a fresh nonce internally (caller-chosen nonces risk catastrophic
    /// AES-GCM reuse) and returns it for [`decrypt`](SharedKey::decrypt).
    #[must_use]
    pub fn encrypt(&self, payload: Vec<u8>) -> Option<(Nonce, Vec<u8>)> {
        let nonce: Nonce = rand::random();
        let cipher_text = self.encrypt_with_nonce(payload, nonce)?;
        Some((nonce, cipher_text))
    }

    /// Encrypts with a caller-supplied nonce. The caller MUST guarantee the nonce
    /// is single-use per key; the sync stream protocol satisfies this via its
    /// per-message `next_nonce` ratchet (see `node/src/sync/blobs.rs`).
    #[must_use]
    pub fn encrypt_with_nonce(&self, payload: Vec<u8>, nonce: Nonce) -> Option<Vec<u8>> {
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
    fn test_encrypt_with_nonce_roundtrip() -> eyre::Result<()> {
        // The sync stream ratchet seals with a caller-chosen nonce and the
        // receiver decrypts with that same nonce; a different nonce must fail.
        let mut csprng = thread_rng();
        let signer = PrivateKey::random(&mut csprng);
        let verifier = PrivateKey::random(&mut csprng);
        let signer_shared_key = SharedKey::new(&signer, &verifier.public_key())?;
        let verifier_shared_key = SharedKey::new(&verifier, &signer.public_key())?;

        let payload = b"privacy is important";
        let nonce: Nonce = rand::random();

        let encrypted = signer_shared_key
            .encrypt_with_nonce(payload.to_vec(), nonce)
            .ok_or_eyre("encryption failed")?;

        assert_eq!(
            verifier_shared_key
                .decrypt(encrypted.clone(), nonce)
                .ok_or_eyre("decryption failed")?,
            payload
        );

        let mut wrong_nonce = nonce;
        wrong_nonce[0] ^= 0x01;
        assert!(
            verifier_shared_key
                .decrypt(encrypted, wrong_nonce)
                .is_none(),
            "decrypt must fail under a nonce other than the one used to seal"
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

#[cfg(test)]
mod x25519_tests {
    use super::*;

    fn secret(seed: u8) -> X25519SecretKey {
        X25519SecretKey::from([seed; 32])
    }

    #[test]
    fn agreement_is_symmetric() {
        // The property scope-key delivery rests on: the sender wraps with its
        // own secret and the recipient's public key, and the recipient unwraps
        // with the mirror pair.
        let (a, b) = (secret(1), secret(2));
        let ab = SharedKey::from_x25519(&a, &b.public_key()).expect("agree");
        let ba = SharedKey::from_x25519(&b, &a.public_key()).expect("agree");

        let (nonce, ct) = ab.encrypt(b"scope key".to_vec()).expect("encrypt");
        assert_eq!(
            ba.decrypt(ct, nonce).as_deref(),
            Some(&b"scope key"[..]),
            "each side must derive the same key"
        );
    }

    #[test]
    fn distinct_peers_derive_distinct_keys() {
        let a = secret(1);
        let with_b = SharedKey::from_x25519(&a, &secret(2).public_key()).expect("agree");
        let with_c = SharedKey::from_x25519(&a, &secret(3).public_key()).expect("agree");

        let (nonce, ct) = with_b.encrypt(b"for b".to_vec()).expect("encrypt");
        assert!(
            with_c.decrypt(ct, nonce).is_none(),
            "a key wrapped for one device must not open with another's"
        );
    }

    #[test]
    fn low_order_public_keys_are_rejected() {
        // A low-order peer key collapses the agreement into a tiny subgroup, so
        // the result stops depending on our scalar. Accepting one would let a
        // peer force a predictable "shared" key.
        let a = secret(1);
        for bytes in [
            [0u8; 32],
            {
                let mut b = [0u8; 32];
                b[0] = 1;
                b
            },
            // Order-8 point from RFC 7748's small-order set.
            [
                0xe0, 0xeb, 0x7a, 0x7c, 0x3b, 0x41, 0xb8, 0xae, 0x16, 0x56, 0xe3, 0xfa, 0xf1, 0x9f,
                0xc4, 0x6a, 0xda, 0x09, 0x8d, 0xeb, 0x9c, 0x32, 0xb1, 0xfd, 0x86, 0x62, 0x05, 0x16,
                0x5f, 0x49, 0xb8, 0x00,
            ],
        ] {
            assert!(
                matches!(
                    SharedKey::from_x25519(&a, &X25519PublicKey::from(bytes)),
                    Err(SharedKeyError::DegenerateAgreement)
                ),
                "low-order key {bytes:?} must be refused"
            );
        }
    }

    #[test]
    fn x25519_and_ed25519_paths_are_domain_separated() {
        // Same 32 secret bytes fed to both derivations must not yield the same
        // AEAD key, so a wrap made for one purpose can never be opened as the
        // other.
        let raw = [7u8; 32];
        let x = SharedKey::from_x25519(&X25519SecretKey::from(raw), &secret(9).public_key())
            .expect("agree");
        let e = SharedKey::new(
            &PrivateKey::from(raw),
            &PrivateKey::from([9u8; 32]).public_key(),
        )
        .expect("agree");

        let (nonce, ct) = x.encrypt(b"payload".to_vec()).expect("encrypt");
        assert!(
            e.decrypt(ct, nonce).is_none(),
            "the two agreement paths must derive different keys"
        );
    }

    #[test]
    fn public_key_derivation_is_deterministic_and_clamped() {
        assert_eq!(secret(5).public_key(), secret(5).public_key());
        assert_ne!(secret(5).public_key(), secret(6).public_key());
        // Clamping happens per use, so the stored secret is unmodified.
        assert_eq!(secret(5).as_bytes(), &[5u8; 32]);
    }

    #[test]
    fn random_secrets_are_distinct() {
        let a = X25519SecretKey::random(&mut rand::thread_rng());
        let b = X25519SecretKey::random(&mut rand::thread_rng());
        assert_ne!(a.public_key(), b.public_key());
    }
}
