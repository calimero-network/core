//! Poker hand evaluation.
//!
//! Evaluates the best 5-card poker hand from 7 cards (2 hole + 5 community).
//!
//! Hand strength is encoded as a single `u32` (`HandScore`) for easy comparison:
//!   `(category << 20) | tiebreaker`
//!
//! Categories (higher = better):
//!   0 = High Card
//!   1 = One Pair
//!   2 = Two Pair
//!   3 = Three of a Kind
//!   4 = Straight
//!   5 = Flush
//!   6 = Full House
//!   7 = Four of a Kind
//!   8 = Straight Flush

use crate::card::Card;

const CATEGORY_SHIFT: u32 = 20;

/// Encoded hand score. Higher value = stronger hand.
///
/// Implements `Ord` so hands can be compared directly.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct HandScore(pub u32);

impl HandScore {
    fn new(category: u32, tiebreaker: u32) -> Self {
        HandScore((category << CATEGORY_SHIFT) | tiebreaker)
    }

    /// Human-readable name for the hand category.
    pub fn category_name(self) -> &'static str {
        match self.0 >> CATEGORY_SHIFT {
            0 => "High Card",
            1 => "One Pair",
            2 => "Two Pair",
            3 => "Three of a Kind",
            4 => "Straight",
            5 => "Flush",
            6 => "Full House",
            7 => "Four of a Kind",
            8 => "Straight Flush",
            _ => "Unknown",
        }
    }
}

/// Evaluate the best 5-card hand from exactly 7 cards.
///
/// Tries all C(7,5) = 21 combinations and returns the highest score.
pub fn evaluate_seven(cards: &[Card; 7]) -> HandScore {
    let mut best = HandScore(0);

    // Enumerate all 21 ways to skip 2 cards from 7
    for skip_a in 0..7 {
        for skip_b in (skip_a + 1)..7 {
            let mut five = [Card(0); 5];
            let mut idx = 0;
            for (k, &card) in cards.iter().enumerate() {
                if k != skip_a && k != skip_b {
                    five[idx] = card;
                    idx += 1;
                }
            }
            let score = evaluate_five(&five);
            if score > best {
                best = score;
            }
        }
    }

    best
}

/// Evaluate a single 5-card poker hand.
pub fn evaluate_five(cards: &[Card; 5]) -> HandScore {
    let mut ranks = [
        cards[0].rank(),
        cards[1].rank(),
        cards[2].rank(),
        cards[3].rank(),
        cards[4].rank(),
    ];
    ranks.sort_unstable_by(|a, b| b.cmp(a)); // descending

    let is_flush = cards[0].suit() == cards[1].suit()
        && cards[1].suit() == cards[2].suit()
        && cards[2].suit() == cards[3].suit()
        && cards[3].suit() == cards[4].suit();

    let (is_straight, straight_high) = check_straight(&ranks);

    // Count rank frequencies
    let mut counts = [0u8; 13];
    for &r in &ranks {
        counts[r as usize] += 1;
    }

    // Classify groups from high rank to low
    let mut quads: Option<u8> = None;
    let mut trips: Option<u8> = None;
    let mut pairs: Vec<u8> = Vec::new();
    let mut singles: Vec<u8> = Vec::new();

    for r in (0..13u8).rev() {
        match counts[r as usize] {
            4 => quads = Some(r),
            3 => trips = Some(r),
            2 => pairs.push(r),
            1 => singles.push(r),
            _ => {}
        }
    }

    // Determine hand category and compute score
    if is_flush && is_straight {
        // Straight Flush (includes Royal Flush when straight_high == 12)
        HandScore::new(8, u32::from(straight_high))
    } else if let Some(q) = quads {
        let kicker = singles.first().or(trips.as_ref()).copied().unwrap_or(0);
        HandScore::new(7, encode_ranks(&[q, kicker]))
    } else if let Some(t) = trips {
        if let Some(&p) = pairs.first() {
            // Full House
            HandScore::new(6, encode_ranks(&[t, p]))
        } else {
            // Three of a Kind
            HandScore::new(3, encode_ranks_with_kickers(&[t], &singles, 2))
        }
    } else if is_flush {
        HandScore::new(5, encode_ranks(&ranks))
    } else if is_straight {
        HandScore::new(4, u32::from(straight_high))
    } else if pairs.len() == 2 {
        // Two Pair
        let kicker = singles.first().copied().unwrap_or(0);
        HandScore::new(2, encode_ranks(&[pairs[0], pairs[1], kicker]))
    } else if pairs.len() == 1 {
        // One Pair
        HandScore::new(1, encode_ranks_with_kickers(&[pairs[0]], &singles, 3))
    } else {
        // High Card
        HandScore::new(0, encode_ranks(&ranks))
    }
}

/// Check if sorted-descending ranks form a straight.
///
/// Returns `(is_straight, high_card)`.
fn check_straight(ranks: &[u8; 5]) -> (bool, u8) {
    // Normal straight: 5 consecutive ranks, all different
    if ranks[0] - ranks[4] == 4
        && ranks[0] != ranks[1]
        && ranks[1] != ranks[2]
        && ranks[2] != ranks[3]
        && ranks[3] != ranks[4]
    {
        return (true, ranks[0]);
    }
    // Ace-low straight (wheel): A-5-4-3-2
    if ranks == &[12, 3, 2, 1, 0] {
        return (true, 3); // 5-high
    }
    (false, 0)
}

/// Pack up to 5 rank values into a `u32` (4 bits each, MSB first).
fn encode_ranks(ranks: &[u8]) -> u32 {
    let mut result = 0u32;
    for (i, &r) in ranks.iter().enumerate().take(5) {
        result |= u32::from(r) << (16 - i * 4);
    }
    result
}

/// Pack primary ranks followed by up to `max_kickers` kicker ranks.
fn encode_ranks_with_kickers(primary: &[u8], kickers: &[u8], max_kickers: usize) -> u32 {
    let mut all: Vec<u8> = primary.to_vec();
    all.extend(kickers.iter().take(max_kickers));
    encode_ranks(&all)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a card from rank (0-12) and suit (0-3).
    fn c(rank: u8, suit: u8) -> Card {
        Card(suit * 13 + rank)
    }

    #[test]
    fn test_royal_flush() {
        let hand = [c(12, 0), c(11, 0), c(10, 0), c(9, 0), c(8, 0)];
        let score = evaluate_five(&hand);
        assert_eq!(score.category_name(), "Straight Flush");
    }

    #[test]
    fn test_ace_low_straight_flush() {
        let hand = [c(12, 2), c(3, 2), c(2, 2), c(1, 2), c(0, 2)];
        let score = evaluate_five(&hand);
        assert_eq!(score.category_name(), "Straight Flush");
        // 5-high, should be less than a 6-high straight flush
        let six_high_sf = evaluate_five(&[c(4, 1), c(3, 1), c(2, 1), c(1, 1), c(0, 1)]);
        assert!(score < six_high_sf);
    }

    #[test]
    fn test_four_of_a_kind() {
        let hand = [c(8, 0), c(8, 1), c(8, 2), c(8, 3), c(12, 0)];
        let score = evaluate_five(&hand);
        assert_eq!(score.category_name(), "Four of a Kind");
    }

    #[test]
    fn test_full_house() {
        let hand = [c(10, 0), c(10, 1), c(10, 2), c(5, 0), c(5, 1)];
        let score = evaluate_five(&hand);
        assert_eq!(score.category_name(), "Full House");
    }

    #[test]
    fn test_flush() {
        let hand = [c(12, 3), c(9, 3), c(7, 3), c(4, 3), c(2, 3)];
        let score = evaluate_five(&hand);
        assert_eq!(score.category_name(), "Flush");
    }

    #[test]
    fn test_straight() {
        let hand = [c(8, 0), c(7, 1), c(6, 2), c(5, 3), c(4, 0)];
        let score = evaluate_five(&hand);
        assert_eq!(score.category_name(), "Straight");
    }

    #[test]
    fn test_three_of_a_kind() {
        let hand = [c(6, 0), c(6, 1), c(6, 2), c(12, 0), c(9, 1)];
        let score = evaluate_five(&hand);
        assert_eq!(score.category_name(), "Three of a Kind");
    }

    #[test]
    fn test_two_pair() {
        let hand = [c(10, 0), c(10, 1), c(5, 0), c(5, 1), c(12, 0)];
        let score = evaluate_five(&hand);
        assert_eq!(score.category_name(), "Two Pair");
    }

    #[test]
    fn test_one_pair() {
        let hand = [c(8, 0), c(8, 1), c(12, 0), c(9, 2), c(4, 3)];
        let score = evaluate_five(&hand);
        assert_eq!(score.category_name(), "One Pair");
    }

    #[test]
    fn test_high_card() {
        let hand = [c(12, 0), c(9, 1), c(7, 2), c(4, 3), c(2, 0)];
        let score = evaluate_five(&hand);
        assert_eq!(score.category_name(), "High Card");
    }

    #[test]
    fn test_hand_ordering() {
        let high = evaluate_five(&[c(12, 0), c(9, 1), c(7, 2), c(4, 3), c(2, 0)]);
        let pair = evaluate_five(&[c(8, 0), c(8, 1), c(12, 0), c(9, 2), c(4, 3)]);
        let two_pair = evaluate_five(&[c(10, 0), c(10, 1), c(5, 0), c(5, 1), c(12, 0)]);
        let trips = evaluate_five(&[c(6, 0), c(6, 1), c(6, 2), c(12, 0), c(9, 1)]);
        let straight = evaluate_five(&[c(8, 0), c(7, 1), c(6, 2), c(5, 3), c(4, 0)]);
        let flush = evaluate_five(&[c(12, 3), c(9, 3), c(7, 3), c(4, 3), c(2, 3)]);
        let full_house = evaluate_five(&[c(10, 0), c(10, 1), c(10, 2), c(5, 0), c(5, 1)]);
        let quads = evaluate_five(&[c(8, 0), c(8, 1), c(8, 2), c(8, 3), c(12, 0)]);
        let straight_flush = evaluate_five(&[c(8, 0), c(7, 0), c(6, 0), c(5, 0), c(4, 0)]);

        assert!(high < pair);
        assert!(pair < two_pair);
        assert!(two_pair < trips);
        assert!(trips < straight);
        assert!(straight < flush);
        assert!(flush < full_house);
        assert!(full_house < quads);
        assert!(quads < straight_flush);
    }

    #[test]
    fn test_seven_card_evaluation() {
        // 7 cards with a flush hiding among them
        let cards = [
            c(12, 3), // As
            c(9, 3),  // Js
            c(7, 3),  // 9s
            c(4, 3),  // 6s
            c(2, 3),  // 4s
            c(10, 0), // Qc — not part of flush
            c(1, 1),  // 3d — not part of flush
        ];
        let score = evaluate_seven(&cards);
        assert_eq!(score.category_name(), "Flush");
    }

    #[test]
    fn test_seven_card_full_house() {
        // Should find Full House from 7 cards
        let cards = [
            c(10, 0),
            c(10, 1),
            c(10, 2),
            c(5, 0),
            c(5, 1),
            c(3, 0),
            c(1, 1),
        ];
        let score = evaluate_seven(&cards);
        assert_eq!(score.category_name(), "Full House");
    }

    #[test]
    fn test_pair_kicker_comparison() {
        // Pair of Aces with K kicker vs pair of Aces with Q kicker
        let aces_king = evaluate_five(&[c(12, 0), c(12, 1), c(11, 2), c(7, 3), c(4, 0)]);
        let aces_queen = evaluate_five(&[c(12, 0), c(12, 1), c(10, 2), c(7, 3), c(4, 0)]);
        assert!(aces_king > aces_queen);
    }
}
