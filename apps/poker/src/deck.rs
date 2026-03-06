//! Deck management with cryptographic shuffling.
//!
//! Uses host-provided randomness (`env::random_bytes`) for a
//! Fisher-Yates shuffle, producing a uniformly random permutation.

use calimero_sdk::env;
use sha2::Digest;

use crate::card::{Card, DECK_SIZE};

/// Creates a new shuffled deck using the Fisher-Yates algorithm
/// with host-provided cryptographic randomness.
///
/// Returns a `Vec<Card>` with 52 cards in random order.
/// Cards are dealt from the end (`.pop()`), so the last element is dealt first.
pub fn new_shuffled_deck() -> Vec<Card> {
    let mut cards: Vec<Card> = (0..DECK_SIZE as u8).map(Card).collect();

    // Need 4 bytes per swap for u32 random values
    let num_swaps = DECK_SIZE - 1;
    let mut random = vec![0u8; num_swaps * 4];
    env::random_bytes(&mut random);

    // Fisher-Yates shuffle (Knuth shuffle)
    for i in (1..DECK_SIZE).rev() {
        let offset = (DECK_SIZE - 1 - i) * 4;
        let r = u32::from_le_bytes([
            random[offset],
            random[offset + 1],
            random[offset + 2],
            random[offset + 3],
        ]);
        let j = (r as usize) % (i + 1);
        cards.swap(i, j);
    }

    cards
}

/// Creates a shuffled deck from a deterministic 32-byte seed.
///
/// Uses SHA-256 to expand the seed into enough randomness for Fisher-Yates.
/// Two calls with the same seed produce the same deck.
pub fn new_shuffled_deck_from_seed(seed: &[u8; 32]) -> Vec<Card> {
    let mut cards: Vec<Card> = (0..DECK_SIZE as u8).map(Card).collect();

    // Expand seed into enough random bytes using SHA-256 chaining
    let num_swaps = DECK_SIZE - 1;
    let mut random = vec![0u8; num_swaps * 4];
    let mut hasher_state = *seed;
    let mut offset = 0;
    while offset < random.len() {
        let hash = sha2::Sha256::digest(hasher_state);
        let copy_len = (random.len() - offset).min(32);
        random[offset..offset + copy_len].copy_from_slice(&hash[..copy_len]);
        hasher_state = hash.into();
        offset += copy_len;
    }

    for i in (1..DECK_SIZE).rev() {
        let off = (DECK_SIZE - 1 - i) * 4;
        let r = u32::from_le_bytes([
            random[off],
            random[off + 1],
            random[off + 2],
            random[off + 3],
        ]);
        let j = (r as usize) % (i + 1);
        cards.swap(i, j);
    }

    cards
}
