//! Card encryption for secure dealing.
//!
//! Uses X25519 ECDH + AES-256-GCM so the dealer can encrypt each player's
//! hole cards with a per-player shared secret.  Only the intended player
//! can decrypt.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use calimero_sdk::env;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

/// Generate a new X25519 keypair using host-provided randomness.
///
/// Returns `(private_key_bytes, public_key_bytes)`.
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
///
/// * `my_secret`    – sender's X25519 private key (32 bytes)
/// * `their_public` – recipient's X25519 public key (32 bytes)
/// * `plaintext`    – card bytes to encrypt (e.g. `[card1, card2]`)
/// * `hand_id`      – used to derive a unique nonce per hand
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
///
/// Returns `None` if decryption fails (wrong key or tampered ciphertext).
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

/// Derive a 12-byte nonce from a hand ID.
fn nonce_from_hand_id(hand_id: u64) -> Nonce<aes_gcm::aead::consts::U12> {
    let mut nonce_bytes = [0u8; 12];
    nonce_bytes[..8].copy_from_slice(&hand_id.to_le_bytes());
    *Nonce::from_slice(&nonce_bytes)
}

/// Hash seeds to produce a deterministic combined seed.
pub fn combine_seeds(seeds: &[Vec<u8>]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for seed in seeds {
        hasher.update(seed);
    }
    hasher.finalize().into()
}

/// Hash a seed for commit-reveal.
pub fn hash_seed(seed: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"calimero-poker-seed:");
    hasher.update(seed);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ecdh_symmetry() {
        // Manually create two keypairs (can't use env::random_bytes in tests)
        let secret_a = [1u8; 32];
        let secret_b = [2u8; 32];
        let public_a = *PublicKey::from(&StaticSecret::from(secret_a)).as_bytes();
        let public_b = *PublicKey::from(&StaticSecret::from(secret_b)).as_bytes();

        let key_ab = derive_aes_key(&secret_a, &public_b);
        let key_ba = derive_aes_key(&secret_b, &public_a);
        assert_eq!(key_ab, key_ba, "ECDH shared secret must be symmetric");
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let secret_a = [1u8; 32];
        let secret_b = [2u8; 32];
        let public_a = *PublicKey::from(&StaticSecret::from(secret_a)).as_bytes();
        let public_b = *PublicKey::from(&StaticSecret::from(secret_b)).as_bytes();

        let cards: &[u8] = &[12, 51]; // Ace of clubs, Ace of spades
        let hand_id = 42u64;

        // A encrypts for B
        let ciphertext = encrypt_cards(&secret_a, &public_b, cards, hand_id);

        // B decrypts from A
        let decrypted = decrypt_cards(&secret_b, &public_a, &ciphertext, hand_id);
        assert_eq!(decrypted, Some(cards.to_vec()));

        // C can't decrypt (wrong key)
        let secret_c = [3u8; 32];
        let decrypted_c = decrypt_cards(&secret_c, &public_a, &ciphertext, hand_id);
        assert_eq!(decrypted_c, None);
    }

    #[test]
    fn test_combine_seeds_deterministic() {
        let seeds = vec![vec![1, 2, 3], vec![4, 5, 6]];
        let h1 = combine_seeds(&seeds);
        let h2 = combine_seeds(&seeds);
        assert_eq!(h1, h2);

        // Different order → different hash
        let seeds_rev = vec![vec![4, 5, 6], vec![1, 2, 3]];
        let h3 = combine_seeds(&seeds_rev);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_hash_seed() {
        let h1 = hash_seed(b"my_secret_seed");
        let h2 = hash_seed(b"my_secret_seed");
        assert_eq!(h1, h2);

        let h3 = hash_seed(b"different_seed");
        assert_ne!(h1, h3);
    }
}
