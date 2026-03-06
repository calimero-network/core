//! Bot strategies for AI poker.
//!
//! Each strategy function takes the current hand state + player position
//! and returns a [`BotAction`] to execute.
//!
//! Strategies:
//!   0 = **Random** — uniformly random legal action
//!   1 = **Caller** — always calls or checks (never folds, never raises)
//!   2 = **TAG** — tight-aggressive: fold trash, raise strong, call medium

use calimero_sdk::env;

use crate::card::Card;
use crate::hand;
use crate::HandState;

/// Action chosen by a bot.
pub enum BotAction {
    Fold,
    Check,
    Call,
    RaiseTo(u64),
}

// ── Strategy 0: Random ──────────────────────────────────────────────

/// Picks a uniformly random legal action.
pub fn random_action(hs: &HandState, pos: usize, bb: u64) -> BotAction {
    let mut rng = [0u8; 1];
    env::random_bytes(&mut rng);

    let can_check = hs.players[pos].bet_this_round >= hs.current_bet;

    match rng[0] % 4 {
        0 => {
            if can_check {
                BotAction::Check
            } else {
                BotAction::Fold
            }
        }
        1 => {
            if can_check {
                BotAction::Check
            } else {
                BotAction::Call
            }
        }
        2 => BotAction::Call,
        _ => BotAction::RaiseTo(hs.current_bet + bb),
    }
}

// ── Strategy 1: Caller ──────────────────────────────────────────────

/// Always calls or checks.  Never folds, never raises.
/// The simplest viable strategy — survives every hand to showdown.
pub fn caller_action(hs: &HandState, pos: usize) -> BotAction {
    if hs.players[pos].bet_this_round >= hs.current_bet {
        BotAction::Check
    } else {
        BotAction::Call
    }
}

// ── Strategy 2: Tight-Aggressive (TAG) ──────────────────────────────

/// Folds weak hands, calls medium, raises strong.
///
/// **Preflop**: uses a simple card-rank heuristic.
/// **Postflop**: evaluates actual hand strength with the poker evaluator.
pub fn tag_action(hs: &HandState, pos: usize, bb: u64) -> BotAction {
    let cards = hs.players[pos].cards;
    let c1 = Card(cards[0]);
    let c2 = Card(cards[1]);
    let can_check = hs.players[pos].bet_this_round >= hs.current_bet;

    if hs.community.is_empty() {
        // ── Preflop heuristic ──
        let score = preflop_score(c1, c2);
        if score >= 8 {
            BotAction::RaiseTo(hs.current_bet + bb * 3)
        } else if score >= 4 {
            if can_check {
                BotAction::Check
            } else {
                BotAction::Call
            }
        } else if can_check {
            BotAction::Check
        } else {
            BotAction::Fold
        }
    } else {
        // ── Postflop: evaluate made hand ──
        let category = postflop_category(c1, c2, &hs.community);
        if category >= 3 {
            // Trips or better → raise
            BotAction::RaiseTo(hs.current_bet + bb * 3)
        } else if category >= 1 {
            // Pair / two-pair → call
            if can_check {
                BotAction::Check
            } else {
                BotAction::Call
            }
        } else if can_check {
            BotAction::Check
        } else {
            BotAction::Fold
        }
    }
}

/// Preflop hand score (0–12).  Higher = stronger.
///
/// Heuristic based on rank, suitedness, and pair status.
fn preflop_score(c1: Card, c2: Card) -> u8 {
    let r1 = c1.rank();
    let r2 = c2.rank();
    let high = r1.max(r2);
    let low = r1.min(r2);
    let suited = c1.suit() == c2.suit();

    // Pocket pair
    if r1 == r2 {
        return high / 2 + 5; // AA=11, KK=10, ..., 22=5
    }

    let mut score: u8 = 0;

    // High card bonus
    if high >= 12 {
        score += 4; // Ace
    } else if high >= 10 {
        score += 3; // Q, K
    } else if high >= 8 {
        score += 2; // T, J
    }

    // Connectedness
    if high - low <= 2 {
        score += 1;
    }

    // Suitedness
    if suited {
        score += 1;
    }

    // Both cards high
    if low >= 8 {
        score += 2;
    }

    score
}

/// Evaluate postflop hand category (0-8) using available cards.
///
/// Uses the full evaluator when 7 cards are available (river),
/// best-of-C(n,5) for 5-6 cards (flop/turn).
fn postflop_category(c1: Card, c2: Card, community: &[u8]) -> u32 {
    let mut all: Vec<Card> = vec![c1, c2];
    for &c in community {
        all.push(Card(c));
    }

    let best_score = match all.len() {
        7 => {
            let seven = [all[0], all[1], all[2], all[3], all[4], all[5], all[6]];
            hand::evaluate_seven(&seven)
        }
        6 => {
            // C(6,5) = 6 combinations
            let mut best = hand::HandScore(0);
            for skip in 0..6 {
                let mut five = [Card(0); 5];
                let mut idx = 0;
                for (k, &card) in all.iter().enumerate() {
                    if k != skip {
                        five[idx] = card;
                        idx += 1;
                    }
                }
                let s = hand::evaluate_five(&five);
                if s > best {
                    best = s;
                }
            }
            best
        }
        5 => {
            let five = [all[0], all[1], all[2], all[3], all[4]];
            hand::evaluate_five(&five)
        }
        _ => hand::HandScore(0),
    };

    // Extract category (top bits)
    best_score.0 >> 20
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflop_aces_are_premium() {
        let ace1 = Card(12); // Ace of clubs
        let ace2 = Card(25); // Ace of diamonds
        assert!(preflop_score(ace1, ace2) >= 8);
    }

    #[test]
    fn preflop_low_offsuit_is_weak() {
        let two = Card(0); // 2c
        let seven = Card(5 + 13); // 7d (different suit)
        assert!(preflop_score(two, seven) < 4);
    }

    #[test]
    fn preflop_suited_connectors_are_medium() {
        let nine_h = Card(7 + 26); // 9h
        let ten_h = Card(8 + 26); // Th
        let score = preflop_score(nine_h, ten_h);
        assert!(score >= 4 && score < 8);
    }

    #[test]
    fn postflop_pair_detected() {
        let ace = Card(12); // Ac
        let king = Card(11); // Kc
        let community = vec![12 + 13, 5, 3]; // Ad, 7c, 5c — paired aces
        let cat = postflop_category(ace, king, &community);
        assert!(cat >= 1, "Should detect at least a pair, got {cat}");
    }

    #[test]
    fn postflop_high_card_only() {
        let two = Card(0); // 2c
        let three = Card(1 + 13); // 3d
        let community = vec![8 + 26, 10, 6 + 13]; // Th, Qc, 8d — no pair
        let cat = postflop_category(two, three, &community);
        assert_eq!(cat, 0, "Should be high card");
    }
}
