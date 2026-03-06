//! PokerBot trait and built-in strategies.
//!
//! Implement [`PokerBot`] to create a custom AI player.
//! The runner calls [`PokerBot::decide`] whenever it's your turn.

use rand::Rng;

use crate::types::GameView;

/// Action a bot can take.
#[derive(Debug, Clone)]
pub enum Action {
    Fold,
    Check,
    Call,
    RaiseTo(u64),
}

/// Trait for AI poker players.
///
/// Implement this to create your own bot strategy.
///
/// ```rust,ignore
/// struct MyBot;
///
/// impl PokerBot for MyBot {
///     fn name(&self) -> &str { "my-bot" }
///     fn decide(&mut self, state: &GameView, my_cards: &[String]) -> Action {
///         Action::Call // simplest possible bot
///     }
/// }
/// ```
pub trait PokerBot {
    /// Display name for this bot.
    fn name(&self) -> &str;

    /// Decide what to do given the public game state and your hole cards.
    fn decide(&mut self, state: &GameView, my_cards: &[String]) -> Action;
}

// ══════════════════════════════════════════════════════════════════════
// Built-in strategies
// ══════════════════════════════════════════════════════════════════════

/// Always calls or checks. Never folds, never raises.
pub struct CallerBot;

impl PokerBot for CallerBot {
    fn name(&self) -> &str {
        "caller"
    }

    fn decide(&mut self, state: &GameView, _my_cards: &[String]) -> Action {
        let me = state
            .players
            .iter()
            .find(|p| p.player_id == state.action_on);
        let my_bet = me.map(|p| p.bet).unwrap_or(0);
        if my_bet >= state.current_bet {
            Action::Check
        } else {
            Action::Call
        }
    }
}

/// Picks a random legal action.
pub struct RandomBot;

impl PokerBot for RandomBot {
    fn name(&self) -> &str {
        "random"
    }

    fn decide(&mut self, state: &GameView, _my_cards: &[String]) -> Action {
        let mut rng = rand::thread_rng();
        let me = state
            .players
            .iter()
            .find(|p| p.player_id == state.action_on);
        let my_bet = me.map(|p| p.bet).unwrap_or(0);
        let can_check = my_bet >= state.current_bet;

        match rng.gen_range(0..4) {
            0 => {
                if can_check {
                    Action::Check
                } else {
                    Action::Fold
                }
            }
            1 => {
                if can_check {
                    Action::Check
                } else {
                    Action::Call
                }
            }
            2 => Action::Call,
            _ => Action::RaiseTo(state.current_bet + state.big_blind),
        }
    }
}

/// Tight-aggressive: fold weak, call medium, raise strong.
///
/// Uses a simple preflop heuristic and postflop board-texture read.
pub struct TagBot;

impl PokerBot for TagBot {
    fn name(&self) -> &str {
        "tag"
    }

    fn decide(&mut self, state: &GameView, my_cards: &[String]) -> Action {
        let me = state
            .players
            .iter()
            .find(|p| p.player_id == state.action_on);
        let my_bet = me.map(|p| p.bet).unwrap_or(0);
        let can_check = my_bet >= state.current_bet;
        let bb = state.big_blind;

        let strength = hand_strength(my_cards, &state.community_cards);

        if strength >= 8 {
            // Strong: raise
            Action::RaiseTo(state.current_bet + bb * 3)
        } else if strength >= 4 {
            // Medium: call / check
            if can_check {
                Action::Check
            } else {
                Action::Call
            }
        } else if can_check {
            // Weak but free: check
            Action::Check
        } else {
            // Weak and facing a bet: fold
            Action::Fold
        }
    }
}

/// Simple hand strength heuristic (0-12).
///
/// Preflop: rank-based scoring.
/// Postflop: checks if cards connect with the board.
fn hand_strength(my_cards: &[String], community: &[String]) -> u8 {
    if my_cards.len() < 2 {
        return 0;
    }

    let r1 = card_rank(&my_cards[0]);
    let r2 = card_rank(&my_cards[1]);
    let high = r1.max(r2);
    let low = r1.min(r2);
    let suited = my_cards[0].chars().last() == my_cards[1].chars().last();
    let paired = r1 == r2;

    if community.is_empty() {
        // Preflop
        let mut score: u8 = 0;
        if paired {
            return high / 2 + 5;
        }
        if high >= 12 {
            score += 4;
        } else if high >= 10 {
            score += 3;
        } else if high >= 8 {
            score += 2;
        }
        if high - low <= 2 {
            score += 1;
        }
        if suited {
            score += 1;
        }
        if low >= 8 {
            score += 2;
        }
        return score;
    }

    // Postflop: check for board hits
    let board_ranks: Vec<u8> = community.iter().map(|c| card_rank(c)).collect();

    let pair_count = [r1, r2]
        .iter()
        .filter(|&&r| board_ranks.contains(&r))
        .count();

    if paired && board_ranks.contains(&r1) {
        return 10; // set
    }
    if paired {
        return 7; // overpair
    }
    if pair_count >= 2 {
        return 9; // two pair
    }
    if pair_count == 1 {
        if high >= 10 {
            return 6;
        } // top pair
        return 4; // mid/low pair
    }
    if high >= 12 {
        return 3;
    } // ace high
    2 // nothing
}

/// Parse card rank from display string like "Ah", "Tc", "2s".
fn card_rank(card: &str) -> u8 {
    match card.chars().next().unwrap_or('?') {
        '2' => 0,
        '3' => 1,
        '4' => 2,
        '5' => 3,
        '6' => 4,
        '7' => 5,
        '8' => 6,
        '9' => 7,
        'T' => 8,
        'J' => 9,
        'Q' => 10,
        'K' => 11,
        'A' => 12,
        _ => 0,
    }
}
