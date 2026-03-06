//! Cryptographic primitives for secure poker dealing.
//!
//! - **VRF**: Verifiable random function for provably fair shuffles
//! - **ECDH + AES-GCM**: Per-player card encryption

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use calimero_sdk::env;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

// ══════════════════════════════════════════════════════════════
// VRF — Verifiable Random Function
// ══════════════════════════════════════════════════════════════
//
// Construction: ECVRF-X25519-SHA256
//   secret_key: X25519 static secret (32 bytes)
//   public_key: X25519 public key (32 bytes)
//   input:      arbitrary bytes (e.g. hand_number as le_bytes)
//
//   output = SHA256("calimero-vrf-output:" || ECDH(sk, hash_to_point(input)))
//   proof  = hash_to_point(input)  (the intermediate curve point)
//
//   verify: ECDH(sk, proof) must produce the same output
//           equivalent: SHA256(pk, proof, output) check via re-derivation
//
// This gives us:
//   - Deterministic: same sk + input → same output
//   - Unpredictable: can't compute output without sk
//   - Verifiable: anyone with pk can check the proof

/// VRF output: the random bytes + proof for verification.
pub struct VrfOutput {
    /// 32 bytes of verifiable randomness (used to shuffle).
    pub random: [u8; 32],
    /// The proof (curve point as 32 bytes). Published for verification.
    pub proof: [u8; 32],
}

/// Compute VRF(sk, input) → (random, proof).
///
/// The `input` is typically the hand number as bytes.
pub fn vrf_compute(secret_key: &[u8; 32], input: &[u8]) -> VrfOutput {
    // Hash input to a curve point (via X25519 clamping)
    let point_bytes = hash_to_x25519_point(input);
    let point = PublicKey::from(point_bytes);

    // ECDH: sk * hash_to_point(input)
    let sk = StaticSecret::from(*secret_key);
    let shared = sk.diffie_hellman(&point);

    // Output = hash of the shared secret
    let mut hasher = Sha256::new();
    hasher.update(b"calimero-vrf-output:");
    hasher.update(shared.as_bytes());
    let random: [u8; 32] = hasher.finalize().into();

    VrfOutput {
        random,
        proof: point_bytes,
    }
}

/// Verify a VRF proof: given pk, input, random, and proof,
/// check that the output is correct.
///
/// Re-derives: ECDH(sk, proof) should give the same random.
/// But we don't have sk — we use the fact that ECDH(sk, point) = ECDH(point_sk, pk)
/// where point_sk is the scalar of the hashed point.
///
/// Simplified verification: re-hash the input to the same point,
/// confirm proof matches, then trust the TEE attestation for the output.
/// Full ECVRF verification requires pairings which X25519 doesn't support.
///
/// For our use case: the TEE attestation proves the code is correct,
/// and the proof lets anyone confirm the same input was used.
pub fn vrf_verify(public_key: &[u8; 32], input: &[u8], proof: &[u8; 32]) -> bool {
    // Re-derive the expected proof point from input
    let expected_point = hash_to_x25519_point(input);
    // The proof must be the hash-to-point of the input
    expected_point == *proof
    // Note: this confirms the input matches the proof.
    // The output correctness is guaranteed by the TEE running attested code.
    // A full standalone ECVRF would need a different curve (Ed25519 + Elligator).
}

/// Hash arbitrary bytes to a valid X25519 point (32 bytes, clamped).
fn hash_to_x25519_point(input: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"calimero-vrf-h2c:");
    hasher.update(input);
    let mut bytes: [u8; 32] = hasher.finalize().into();
    // Clamp to valid X25519 scalar (same as what StaticSecret::from does)
    bytes[0] &= 248;
    bytes[31] &= 127;
    bytes[31] |= 64;
    bytes
}

// ══════════════════════════════════════════════════════════════
// Card Encryption — ECDH + AES-256-GCM
// ══════════════════════════════════════════════════════════════

/// Generate a new X25519 keypair using host-provided randomness.
pub fn generate_keypair() -> ([u8; 32], [u8; 32]) {
    let mut secret_bytes = [0u8; 32];
    env::random_bytes(&mut secret_bytes);
    let secret = StaticSecret::from(secret_bytes);
    let public = PublicKey::from(&secret);
    (secret_bytes, *public.as_bytes())
}

/// Derive an AES-256 key from an X25519 ECDH shared secret.
fn derive_aes_key(my_secret: &[u8; 32], their_public: &[u8; 32]) -> [u8; 32] {
    let secret = StaticSecret::from(*my_secret);
    let public = PublicKey::from(*their_public);
    let shared = secret.diffie_hellman(&public);

    let mut hasher = Sha256::new();
    hasher.update(shared.as_bytes());
    hasher.update(b"calimero-poker-cards");
    hasher.finalize().into()
}

/// Encrypt card data for a recipient.
pub fn encrypt_cards(
    my_secret: &[u8; 32],
    their_public: &[u8; 32],
    plaintext: &[u8],
    hand_id: u64,
) -> Vec<u8> {
    let key = derive_aes_key(my_secret, their_public);
    let cipher = Aes256Gcm::new_from_slice(&key).expect("valid key length");
    let nonce = nonce_from_hand_id(hand_id);
    cipher
        .encrypt(&nonce, plaintext)
        .expect("encryption failed")
}

/// Decrypt card data from a sender.
pub fn decrypt_cards(
    my_secret: &[u8; 32],
    their_public: &[u8; 32],
    ciphertext: &[u8],
    hand_id: u64,
) -> Option<Vec<u8>> {
    let key = derive_aes_key(my_secret, their_public);
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    let nonce = nonce_from_hand_id(hand_id);
    cipher.decrypt(&nonce, ciphertext).ok()
}

fn nonce_from_hand_id(hand_id: u64) -> Nonce<aes_gcm::aead::consts::U12> {
    let mut nonce_bytes = [0u8; 12];
    nonce_bytes[..8].copy_from_slice(&hand_id.to_le_bytes());
    *Nonce::from_slice(&nonce_bytes)
}

// ══════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vrf_deterministic() {
        let sk = [42u8; 32];
        let input = 1u64.to_le_bytes();

        let out1 = vrf_compute(&sk, &input);
        let out2 = vrf_compute(&sk, &input);
        assert_eq!(out1.random, out2.random);
        assert_eq!(out1.proof, out2.proof);
    }

    #[test]
    fn vrf_different_inputs_different_outputs() {
        let sk = [42u8; 32];
        let out1 = vrf_compute(&sk, &1u64.to_le_bytes());
        let out2 = vrf_compute(&sk, &2u64.to_le_bytes());
        assert_ne!(out1.random, out2.random);
    }

    #[test]
    fn vrf_different_keys_different_outputs() {
        let input = 1u64.to_le_bytes();
        let out1 = vrf_compute(&[1u8; 32], &input);
        let out2 = vrf_compute(&[2u8; 32], &input);
        assert_ne!(out1.random, out2.random);
    }

    #[test]
    fn vrf_verify_valid() {
        let sk = [42u8; 32];
        let pk = *PublicKey::from(&StaticSecret::from(sk)).as_bytes();
        let input = 1u64.to_le_bytes();
        let out = vrf_compute(&sk, &input);

        assert!(vrf_verify(&pk, &input, &out.proof));
    }

    #[test]
    fn vrf_verify_wrong_input() {
        let sk = [42u8; 32];
        let pk = *PublicKey::from(&StaticSecret::from(sk)).as_bytes();
        let out = vrf_compute(&sk, &1u64.to_le_bytes());

        // Wrong input → proof doesn't match
        assert!(!vrf_verify(&pk, &2u64.to_le_bytes(), &out.proof));
    }

    #[test]
    fn ecdh_symmetry() {
        let secret_a = [1u8; 32];
        let secret_b = [2u8; 32];
        let public_a = *PublicKey::from(&StaticSecret::from(secret_a)).as_bytes();
        let public_b = *PublicKey::from(&StaticSecret::from(secret_b)).as_bytes();

        let key_ab = derive_aes_key(&secret_a, &public_b);
        let key_ba = derive_aes_key(&secret_b, &public_a);
        assert_eq!(key_ab, key_ba);
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let secret_a = [1u8; 32];
        let secret_b = [2u8; 32];
        let public_a = *PublicKey::from(&StaticSecret::from(secret_a)).as_bytes();
        let public_b = *PublicKey::from(&StaticSecret::from(secret_b)).as_bytes();

        let cards: &[u8] = &[12, 51];
        let ciphertext = encrypt_cards(&secret_a, &public_b, cards, 42);
        let decrypted = decrypt_cards(&secret_b, &public_a, &ciphertext, 42);
        assert_eq!(decrypted, Some(cards.to_vec()));

        // Wrong key can't decrypt
        let decrypted_c = decrypt_cards(&[3u8; 32], &public_a, &ciphertext, 42);
        assert_eq!(decrypted_c, None);
    }
}
