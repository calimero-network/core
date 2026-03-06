//! Types matching the Calimero poker WASM JSON-RPC responses.

#![allow(dead_code)] // Fields deserialized from JSON but not all read directly

use serde::Deserialize;

/// Public game state returned by `get_game_state()`.
#[derive(Debug, Clone, Deserialize)]
pub struct GameView {
    pub phase: String,
    pub community_cards: Vec<String>,
    pub pot: u64,
    pub current_bet: u64,
    pub hand_number: u64,
    pub dealer_seat: i8,
    pub action_on: String,
    pub players: Vec<PlayerView>,
    pub small_blind: u64,
    pub big_blind: u64,
}

/// Per-player info visible in the game state.
#[derive(Debug, Clone, Deserialize)]
pub struct PlayerView {
    pub player_id: String,
    pub seat: u8,
    pub chips: u64,
    pub bet: u64,
    pub folded: bool,
    pub all_in: bool,
    pub in_hand: bool,
}

/// A player's revealed cards from a completed hand.
#[derive(Debug, Clone, Deserialize)]
pub struct RevealedHand {
    pub player_id: String,
    pub card1: String,
    pub card2: String,
}

/// Result of a completed hand.
#[derive(Debug, Clone, Deserialize)]
pub struct HandResult {
    pub hand_number: u64,
    pub winner_id: String,
    pub winning_hand: String,
    pub pot: u64,
    pub reason: String,
    pub player_cards: Vec<RevealedHand>,
    pub community_cards: Vec<String>,
}

/// Table statistics.
#[derive(Debug, Clone, Deserialize)]
pub struct TableStats {
    pub hands_played: u64,
    pub players: Vec<PlayerStats>,
}

/// Per-player aggregate stats.
#[derive(Debug, Clone, Deserialize)]
pub struct PlayerStats {
    pub player_id: String,
    pub wins: u64,
    pub chips: u64,
}
