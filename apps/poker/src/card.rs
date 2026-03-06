//! Card representation for poker.
//!
//! Cards are represented as u8 values 0-51:
//! - Rank = card % 13 (0=Two, 1=Three, ..., 12=Ace)
//! - Suit = card / 13 (0=Clubs, 1=Diamonds, 2=Hearts, 3=Spades)

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};

pub const DECK_SIZE: usize = 52;
pub const NUM_RANKS: u8 = 13;

/// A playing card encoded as a single byte (0-51).
#[derive(Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize, Debug)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Card(pub u8);

impl Card {
    /// The rank of the card (0=Two through 12=Ace).
    pub fn rank(self) -> u8 {
        self.0 % NUM_RANKS
    }

    /// The suit of the card (0=Clubs, 1=Diamonds, 2=Hearts, 3=Spades).
    pub fn suit(self) -> u8 {
        self.0 / NUM_RANKS
    }
}

impl core::fmt::Display for Card {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let rank = match self.rank() {
            0 => "2",
            1 => "3",
            2 => "4",
            3 => "5",
            4 => "6",
            5 => "7",
            6 => "8",
            7 => "9",
            8 => "T",
            9 => "J",
            10 => "Q",
            11 => "K",
            12 => "A",
            _ => "?",
        };
        let suit = match self.suit() {
            0 => "c",
            1 => "d",
            2 => "h",
            3 => "s",
            _ => "?",
        };
        write!(f, "{rank}{suit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_card_encoding() {
        // Two of Clubs = 0*13 + 0 = 0
        let two_clubs = Card(0);
        assert_eq!(two_clubs.rank(), 0);
        assert_eq!(two_clubs.suit(), 0);
        assert_eq!(two_clubs.to_string(), "2c");

        // Ace of Spades = 3*13 + 12 = 51
        let ace_spades = Card(51);
        assert_eq!(ace_spades.rank(), 12);
        assert_eq!(ace_spades.suit(), 3);
        assert_eq!(ace_spades.to_string(), "As");

        // King of Hearts = 2*13 + 11 = 37
        let king_hearts = Card(37);
        assert_eq!(king_hearts.rank(), 11);
        assert_eq!(king_hearts.suit(), 2);
        assert_eq!(king_hearts.to_string(), "Kh");
    }
}
