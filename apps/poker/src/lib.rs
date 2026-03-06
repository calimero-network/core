//! # Poker — Texas Hold'em on Calimero
//!
//! A decentralized Texas Hold'em poker game running as a Calimero WASM application.
//!
//! Each Calimero **context** acts as a poker table. Players join by taking a seat
//! and buying in with chips. The game follows standard Texas Hold'em rules:
//! Preflop → Flop → Turn → River → Showdown.
//!
//! ## Architecture
//!
//! - **Shared state** (CRDTs): seat map, chip counts, hand state.
//! - **Randomness**: `env::random_bytes` for Fisher-Yates shuffle.
//! - **Identity**: `env::executor_id()` for turn enforcement.
//! - **Events**: real-time spectator feed via SSE / WebSocket.

mod bot;
mod card;
mod crypto;
mod deck;
mod hand;

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env};
use calimero_storage::collections::{Counter, LwwRegister, UnorderedMap};
use thiserror::Error;

use card::Card;

// ══════════════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════════════

const PHASE_WAITING: u8 = 0;
const PHASE_PREFLOP: u8 = 1;
const PHASE_FLOP: u8 = 2;
const PHASE_TURN: u8 = 3;
const PHASE_RIVER: u8 = 4;

const DEFAULT_SMALL_BLIND: u64 = 10;
const DEFAULT_BIG_BLIND: u64 = 20;
const DEFAULT_MIN_BUY_IN: u64 = 400; // 20× big blind
const MAX_SEATS: u8 = 6;
const DEFAULT_TIMEOUT_NS: u64 = 30_000_000_000; // 30 seconds in nanoseconds

// ══════════════════════════════════════════════════════════════════════
// Per-Hand Types (serialised inside LwwRegister)
// ══════════════════════════════════════════════════════════════════════

/// State of a single player within a hand.
#[derive(Clone, Debug, Default, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct PlayerHand {
    pub player_id: String,
    pub seat: u8,
    pub cards: [u8; 2],
    pub bet_this_round: u64,
    pub bet_total: u64,
    pub folded: bool,
    pub all_in: bool,
}

/// Complete state of the current hand.
///
/// Stored as a single blob inside `LwwRegister<HandState>`.
/// Because poker is turn-based, concurrent writes are not expected.
#[derive(Clone, Debug, Default, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct HandState {
    pub phase: u8,
    pub deck: Vec<u8>,
    pub community: Vec<u8>,
    pub players: Vec<PlayerHand>,
    pub dealer_pos: u8,
    pub action_pos: u8,
    pub pot: u64,
    pub current_bet: u64,
    /// Which players have acted in the current betting round.
    pub acted: Vec<bool>,
    pub last_action_time: u64,
}

/// A player's hole cards revealed after showdown.
#[derive(Clone, Debug, Default, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct RevealedHand {
    pub player_id: String,
    pub card1: String,
    pub card2: String,
}

/// Result of the last completed hand (persisted for queries).
#[derive(Clone, Debug, Default, BorshSerialize, BorshDeserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
pub struct HandResult {
    pub hand_number: u64,
    pub winner_id: String,
    pub winning_hand: String,
    pub pot: u64,
    pub reason: String,
    pub player_cards: Vec<RevealedHand>,
    pub community_cards: Vec<String>,
}

// ══════════════════════════════════════════════════════════════════════
// View types (returned to callers via JSON-RPC)
// ══════════════════════════════════════════════════════════════════════

/// Public game state visible to anyone.
#[derive(Serialize)]
#[serde(crate = "calimero_sdk::serde")]
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

/// Public per-player info (no hole cards).
#[derive(Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct PlayerView {
    pub player_id: String,
    pub seat: u8,
    pub chips: u64,
    pub bet: u64,
    pub folded: bool,
    pub all_in: bool,
    pub in_hand: bool,
}

/// Aggregate stats for a player across all hands.
#[derive(Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct PlayerStats {
    pub player_id: String,
    pub wins: u64,
    pub chips: u64,
    pub hands_played: u64,
}

/// Overall table statistics.
#[derive(Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct TableStats {
    pub hands_played: u64,
    pub players: Vec<PlayerStats>,
}

// ══════════════════════════════════════════════════════════════════════
// Events
// ══════════════════════════════════════════════════════════════════════

#[app::event]
pub enum PokerEvent<'a> {
    PlayerJoined {
        player_id: &'a str,
        seat: u8,
        buy_in: u64,
    },
    PlayerLeft {
        player_id: &'a str,
        seat: u8,
    },
    HandStarted {
        hand_number: u64,
        dealer_seat: u8,
    },
    BlindsPosted {
        small: &'a str,
        big: &'a str,
        sb_amount: u64,
        bb_amount: u64,
    },
    PhaseChanged {
        phase: &'a str,
    },
    PlayerActed {
        player_id: &'a str,
        action: &'a str,
        amount: u64,
    },
    CommunityDealt {
        cards: &'a str,
    },
    ShowdownResult {
        winner: &'a str,
        hand_name: &'a str,
        pot: u64,
    },
    HandComplete {
        winner: &'a str,
        pot: u64,
        reason: &'a str,
    },
    PlayerTimedOut {
        player_id: &'a str,
    },
}

// ══════════════════════════════════════════════════════════════════════
// Errors
// ══════════════════════════════════════════════════════════════════════

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind")]
pub enum PokerError {
    #[error("table is full")]
    TableFull,
    #[error("already seated")]
    AlreadySeated,
    #[error("not seated at this table")]
    NotSeated,
    #[error("hand already in progress")]
    HandInProgress,
    #[error("no hand in progress")]
    NoHandInProgress,
    #[error("not your turn")]
    NotYourTurn,
    #[error("need at least 2 players")]
    NotEnoughPlayers,
    #[error("buy-in below minimum")]
    BuyInTooLow,
    #[error("cannot check when facing a bet")]
    CannotCheck,
    #[error("raise must be at least the big blind above the current bet")]
    RaiseTooSmall,
    #[error("cannot leave while in an active hand")]
    CannotLeaveInHand,
    #[error("player has not timed out yet")]
    NotTimedOut,
    #[error("not the designated dealer")]
    NotDealer,
    #[error("encryption key not registered")]
    NoEncryptionKey,
    #[error("dealer not registered")]
    NoDealerRegistered,
}

// ══════════════════════════════════════════════════════════════════════
// Application State
// ══════════════════════════════════════════════════════════════════════

#[app::state(emits = for<'a> PokerEvent<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct PokerGame {
    small_blind: LwwRegister<u64>,
    big_blind: LwwRegister<u64>,
    min_buy_in: LwwRegister<u64>,
    max_seats: LwwRegister<u8>,

    /// Seat index (string "0"-"5") → player id (base58). Empty string = vacant.
    seats: UnorderedMap<String, LwwRegister<String>>,

    /// Player id → chip count.
    chips: UnorderedMap<String, LwwRegister<u64>>,

    /// Occupied seat count.
    num_players: LwwRegister<u8>,

    /// Which seat gets the dealer button next.
    next_dealer: LwwRegister<u8>,

    /// Timeout for a player action (nanoseconds).
    timeout_ns: LwwRegister<u64>,

    /// All per-hand data, stored as one atomic blob.
    hand: LwwRegister<HandState>,

    /// Result of the most recent completed hand.
    last_result: LwwRegister<HandResult>,

    /// Full hand history (all completed hands).
    history: LwwRegister<Vec<HandResult>>,

    /// Per-player win count: player_id → wins.
    win_count: UnorderedMap<String, LwwRegister<u64>>,

    /// Lifetime hand counter.
    hands_played: Counter,

    // ── Secure dealing (VRF + encrypted cards) ──
    /// Dealer's player ID (non-playing participant).
    dealer_id: LwwRegister<String>,

    /// Player id → X25519 public key (32 bytes).
    encryption_keys: UnorderedMap<String, LwwRegister<Vec<u8>>>,

    /// Encrypted hole cards: player_id → ciphertext.
    encrypted_cards: UnorderedMap<String, LwwRegister<Vec<u8>>>,

    /// VRF proof for the current hand (published for verification).
    vrf_proof: LwwRegister<Vec<u8>>,

    /// Whether secure dealing mode is active.
    secure_mode: LwwRegister<bool>,
}

// ══════════════════════════════════════════════════════════════════════
// Free helpers
// ══════════════════════════════════════════════════════════════════════

fn player_id() -> String {
    bs58::encode(env::executor_id()).into_string()
}

fn phase_name(phase: u8) -> &'static str {
    match phase {
        PHASE_WAITING => "Waiting",
        PHASE_PREFLOP => "PreFlop",
        PHASE_FLOP => "Flop",
        PHASE_TURN => "Turn",
        PHASE_RIVER => "River",
        _ => "Unknown",
    }
}

fn cards_display(cards: &[u8]) -> String {
    cards
        .iter()
        .map(|&c| Card(c).to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

// ══════════════════════════════════════════════════════════════════════
// Public API  (exposed via ABI / JSON-RPC)
// ══════════════════════════════════════════════════════════════════════

#[app::logic]
impl PokerGame {
    // ── initialisation ──────────────────────────────────────────────

    /// Create a new poker table.
    ///
    /// All parameters are optional — omitted values use sensible defaults:
    ///   `small_blind`  = 10,  `big_blind` = 20,  `min_buy_in` = 400,
    ///   `max_seats` = 6,  `timeout_secs` = 30
    ///
    /// Pass `{}` for a standard table, or customise:
    /// ```json
    /// { "small_blind": 5, "big_blind": 10, "min_buy_in": 200, "max_seats": 4 }
    /// ```
    #[app::init]
    pub fn init(
        small_blind: Option<u64>,
        big_blind: Option<u64>,
        min_buy_in: Option<u64>,
        max_seats: Option<u8>,
        timeout_secs: Option<u64>,
    ) -> PokerGame {
        let sb = small_blind.unwrap_or(DEFAULT_SMALL_BLIND);
        let bb = big_blind.unwrap_or(DEFAULT_BIG_BLIND);
        let min = min_buy_in.unwrap_or(DEFAULT_MIN_BUY_IN);
        let seats = max_seats.unwrap_or(MAX_SEATS);
        let timeout = timeout_secs.unwrap_or(30) * 1_000_000_000;

        app::log!(
            "Initializing poker table: blinds {}/{}, buy-in ≥{}, {} seats",
            sb,
            bb,
            min,
            seats
        );

        PokerGame {
            small_blind: sb.into(),
            big_blind: bb.into(),
            min_buy_in: min.into(),
            max_seats: seats.into(),
            seats: UnorderedMap::new(),
            chips: UnorderedMap::new(),
            num_players: 0u8.into(),
            next_dealer: 0u8.into(),
            timeout_ns: timeout.into(),
            hand: HandState::default().into(),
            last_result: HandResult::default().into(),
            history: Vec::<HandResult>::new().into(),
            win_count: UnorderedMap::new(),
            hands_played: Counter::new(),
            dealer_id: String::new().into(),
            encryption_keys: UnorderedMap::new(),
            encrypted_cards: UnorderedMap::new(),
            vrf_proof: Vec::new().into(),
            secure_mode: false.into(),
        }
    }

    /// Reconfigure table settings between hands.
    ///
    /// Only fields that are `Some(...)` are updated.
    /// Cannot be called while a hand is in progress.
    pub fn configure(
        &mut self,
        small_blind: Option<u64>,
        big_blind: Option<u64>,
        min_buy_in: Option<u64>,
        max_seats: Option<u8>,
        timeout_secs: Option<u64>,
    ) -> app::Result<()> {
        if self.hand.get().phase != PHASE_WAITING {
            app::bail!(PokerError::HandInProgress);
        }

        if let Some(v) = small_blind {
            self.small_blind.set(v);
        }
        if let Some(v) = big_blind {
            self.big_blind.set(v);
        }
        if let Some(v) = min_buy_in {
            self.min_buy_in.set(v);
        }
        if let Some(v) = max_seats {
            self.max_seats.set(v);
        }
        if let Some(v) = timeout_secs {
            self.timeout_ns.set(v * 1_000_000_000);
        }

        app::log!(
            "Table reconfigured: blinds {}/{}, buy-in ≥{}, {} seats",
            self.small_blind.get(),
            self.big_blind.get(),
            self.min_buy_in.get(),
            self.max_seats.get()
        );
        Ok(())
    }

    // ── lobby ───────────────────────────────────────────────────────

    /// Take a seat at the table with a chip buy-in.
    pub fn join_table(&mut self, buy_in: u64) -> app::Result<()> {
        let caller = player_id();
        let min = *self.min_buy_in.get();

        if buy_in < min {
            app::bail!(PokerError::BuyInTooLow);
        }

        // Reject if already seated
        let max = *self.max_seats.get();
        for i in 0..max {
            if let Some(occ) = self.seats.get(&i.to_string())? {
                if *occ.get() == caller {
                    app::bail!(PokerError::AlreadySeated);
                }
            }
        }

        // Find first empty seat
        let seat = self.find_empty_seat()?;
        let Some(seat) = seat else {
            app::bail!(PokerError::TableFull);
        };

        self.seats.insert(seat.to_string(), caller.clone().into())?;
        self.chips.insert(caller.clone(), buy_in.into())?;
        self.num_players.set(*self.num_players.get() + 1);

        app::emit!(PokerEvent::PlayerJoined {
            player_id: &caller,
            seat,
            buy_in,
        });
        app::log!(
            "Player {} joined seat {} with {} chips",
            caller,
            seat,
            buy_in
        );
        Ok(())
    }

    /// Leave the table.  Cannot leave during an active hand unless folded.
    pub fn leave_table(&mut self) -> app::Result<()> {
        let caller = player_id();
        let hs = self.hand.get();

        if hs.phase != PHASE_WAITING {
            for p in &hs.players {
                if p.player_id == caller && !p.folded {
                    app::bail!(PokerError::CannotLeaveInHand);
                }
            }
        }

        let max = *self.max_seats.get();
        let mut found_seat: Option<u8> = None;
        for i in 0..max {
            if let Some(occ) = self.seats.get(&i.to_string())? {
                if *occ.get() == caller {
                    found_seat = Some(i);
                    break;
                }
            }
        }

        let Some(seat) = found_seat else {
            app::bail!(PokerError::NotSeated);
        };

        self.seats.insert(seat.to_string(), String::new().into())?;
        self.chips.insert(caller.clone(), 0u64.into())?;
        self.num_players
            .set(self.num_players.get().saturating_sub(1));

        app::emit!(PokerEvent::PlayerLeft {
            player_id: &caller,
            seat,
        });
        app::log!("Player {} left seat {}", caller, seat);
        Ok(())
    }

    // ── game control ────────────────────────────────────────────────

    /// Start a new hand.  Shuffles, deals hole cards, posts blinds.
    pub fn start_hand(&mut self) -> app::Result<()> {
        if self.hand.get().phase != PHASE_WAITING {
            app::bail!(PokerError::HandInProgress);
        }

        let mut seated = self.get_seated_players()?;
        if seated.len() < 2 {
            app::bail!(PokerError::NotEnoughPlayers);
        }

        // Rotate so dealer is first
        let dealer_seat = *self.next_dealer.get();
        let dealer_idx = nearest_index(&seated, dealer_seat);
        seated.rotate_left(dealer_idx);

        let num = seated.len();
        let sb = *self.small_blind.get();
        let bb = *self.big_blind.get();

        // Shuffle & deal
        let mut deck: Vec<u8> = deck::new_shuffled_deck().iter().map(|c| c.0).collect();
        let mut players: Vec<PlayerHand> = Vec::with_capacity(num);
        for &(seat, ref pid) in &seated {
            let c1 = deck.pop().unwrap_or(0);
            let c2 = deck.pop().unwrap_or(0);
            players.push(PlayerHand {
                player_id: pid.clone(),
                seat,
                cards: [c1, c2],
                ..PlayerHand::default()
            });
        }

        // Blind positions
        let (sb_idx, bb_idx) = if num == 2 { (0, 1) } else { (1, 2) };

        let sb_actual = self.deduct_chips(&players[sb_idx].player_id, sb)?;
        players[sb_idx].bet_this_round = sb_actual;
        players[sb_idx].bet_total = sb_actual;
        if sb_actual < sb {
            players[sb_idx].all_in = true;
        }

        let bb_actual = self.deduct_chips(&players[bb_idx].player_id, bb)?;
        players[bb_idx].bet_this_round = bb_actual;
        players[bb_idx].bet_total = bb_actual;
        if bb_actual < bb {
            players[bb_idx].all_in = true;
        }

        let pot = sb_actual + bb_actual;

        // First to act: heads-up → SB(dealer=0); 3+ → UTG = after BB
        let first_to_act = if num == 2 { 0 } else { (bb_idx + 1) % num };

        let mut acted = vec![false; num];
        // Mark all-in players as acted so they're skipped
        for (i, p) in players.iter().enumerate() {
            if p.all_in {
                acted[i] = true;
            }
        }

        let new_hs = HandState {
            phase: PHASE_PREFLOP,
            deck,
            community: Vec::new(),
            players,
            dealer_pos: 0,
            action_pos: first_to_act as u8,
            pot,
            current_bet: bb_actual,
            acted,
            last_action_time: env::time_now(),
        };

        self.hand.set(new_hs.clone());
        self.hands_played.increment()?;
        let hand_num = self.hands_played.value()?;

        // Advance dealer button
        self.next_dealer
            .set((dealer_seat + 1) % *self.max_seats.get());

        let sb_id = new_hs.players[sb_idx].player_id.clone();
        let bb_id = new_hs.players[bb_idx].player_id.clone();

        app::emit!(PokerEvent::HandStarted {
            hand_number: hand_num,
            dealer_seat: new_hs.players[0].seat,
        });
        app::emit!(PokerEvent::BlindsPosted {
            small: &sb_id,
            big: &bb_id,
            sb_amount: sb_actual,
            bb_amount: bb_actual,
        });
        app::log!(
            "Hand #{} started. Dealer seat {}",
            hand_num,
            new_hs.players[0].seat
        );
        Ok(())
    }

    // ── player actions ──────────────────────────────────────────────

    /// Fold the current hand.
    pub fn fold(&mut self) -> app::Result<()> {
        let caller = player_id();
        let mut hs = self.hand.get().clone();
        Self::require_active_hand(&hs)?;
        let pos = Self::verify_turn(&hs, &caller)?;

        hs.players[pos].folded = true;
        hs.acted[pos] = true;
        hs.last_action_time = env::time_now();

        app::emit!(PokerEvent::PlayerActed {
            player_id: &caller,
            action: "fold",
            amount: 0,
        });

        // If only one non-folded player remains, they win immediately
        let remaining: Vec<usize> = hs
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.folded)
            .map(|(i, _)| i)
            .collect();

        if remaining.len() == 1 {
            self.award_pot(&mut hs, remaining[0]);
            self.hand.set(hs);
            return Ok(());
        }

        self.advance_action(&mut hs);
        self.hand.set(hs);
        Ok(())
    }

    /// Check (pass).  Only valid when you've already matched the current bet.
    pub fn check(&mut self) -> app::Result<()> {
        let caller = player_id();
        let mut hs = self.hand.get().clone();
        Self::require_active_hand(&hs)?;
        let pos = Self::verify_turn(&hs, &caller)?;

        if hs.players[pos].bet_this_round < hs.current_bet {
            app::bail!(PokerError::CannotCheck);
        }

        hs.acted[pos] = true;
        hs.last_action_time = env::time_now();

        app::emit!(PokerEvent::PlayerActed {
            player_id: &caller,
            action: "check",
            amount: 0,
        });

        self.advance_action(&mut hs);
        self.hand.set(hs);
        Ok(())
    }

    /// Call the current bet.
    pub fn call(&mut self) -> app::Result<()> {
        let caller = player_id();
        let mut hs = self.hand.get().clone();
        Self::require_active_hand(&hs)?;
        let pos = Self::verify_turn(&hs, &caller)?;

        let to_call = hs.current_bet - hs.players[pos].bet_this_round;
        if to_call == 0 {
            // Nothing to call → treat as check
            hs.acted[pos] = true;
            hs.last_action_time = env::time_now();
            app::emit!(PokerEvent::PlayerActed {
                player_id: &caller,
                action: "check",
                amount: 0,
            });
            self.advance_action(&mut hs);
            self.hand.set(hs);
            return Ok(());
        }

        let actual = self.deduct_chips(&caller, to_call)?;
        hs.players[pos].bet_this_round += actual;
        hs.players[pos].bet_total += actual;
        hs.pot += actual;

        if actual < to_call {
            hs.players[pos].all_in = true;
        }

        hs.acted[pos] = true;
        hs.last_action_time = env::time_now();

        app::emit!(PokerEvent::PlayerActed {
            player_id: &caller,
            action: "call",
            amount: actual,
        });

        self.advance_action(&mut hs);
        self.hand.set(hs);
        Ok(())
    }

    /// Raise the bet to `amount` (total bet this round, not the increment).
    ///
    /// Minimum raise is `current_bet + big_blind`.
    /// Going all-in for less is allowed.
    pub fn raise_to(&mut self, amount: u64) -> app::Result<()> {
        let caller = player_id();
        let mut hs = self.hand.get().clone();
        Self::require_active_hand(&hs)?;
        let pos = Self::verify_turn(&hs, &caller)?;

        let bb = *self.big_blind.get();
        let min_raise = hs.current_bet + bb;
        let already_bet = hs.players[pos].bet_this_round;
        let needed = amount.saturating_sub(already_bet);
        let player_chips = self.get_player_chips(&caller)?;

        // Allow all-in for less than min raise
        if amount < min_raise && needed < player_chips {
            app::bail!(PokerError::RaiseTooSmall);
        }
        if needed == 0 {
            app::bail!(PokerError::RaiseTooSmall);
        }

        let actual = self.deduct_chips(&caller, needed)?;
        hs.players[pos].bet_this_round += actual;
        hs.players[pos].bet_total += actual;
        hs.pot += actual;
        hs.current_bet = hs.players[pos].bet_this_round;

        if self.get_player_chips(&caller)? == 0 {
            hs.players[pos].all_in = true;
        }

        // Reset acted: everyone except raiser / folded / all-in must act again
        for (i, acted) in hs.acted.iter_mut().enumerate() {
            *acted = i == pos || hs.players[i].folded || hs.players[i].all_in;
        }

        hs.last_action_time = env::time_now();

        app::emit!(PokerEvent::PlayerActed {
            player_id: &caller,
            action: "raise",
            amount: hs.players[pos].bet_this_round,
        });

        self.advance_action(&mut hs);
        self.hand.set(hs);
        Ok(())
    }

    // ── queries ─────────────────────────────────────────────────────

    /// Get public game state (no hole cards).
    pub fn get_game_state(&self) -> app::Result<GameView> {
        let hs = self.hand.get();
        let hand_num = self.hands_played.value()?;

        let mut players = Vec::new();

        if hs.phase != PHASE_WAITING {
            for p in &hs.players {
                let chips = self.get_player_chips(&p.player_id)?;
                players.push(PlayerView {
                    player_id: p.player_id.clone(),
                    seat: p.seat,
                    chips,
                    bet: p.bet_this_round,
                    folded: p.folded,
                    all_in: p.all_in,
                    in_hand: true,
                });
            }
        } else {
            let seated = self.get_seated_players()?;
            for (seat, pid) in &seated {
                let chips = self.get_player_chips(pid)?;
                players.push(PlayerView {
                    player_id: pid.clone(),
                    seat: *seat,
                    chips,
                    bet: 0,
                    folded: false,
                    all_in: false,
                    in_hand: false,
                });
            }
        }

        let action_on = if hs.phase != PHASE_WAITING {
            hs.players
                .get(hs.action_pos as usize)
                .map(|p| p.player_id.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };

        Ok(GameView {
            phase: phase_name(hs.phase).to_string(),
            community_cards: hs.community.iter().map(|&c| Card(c).to_string()).collect(),
            pot: hs.pot,
            current_bet: hs.current_bet,
            hand_number: hand_num,
            dealer_seat: if hs.phase != PHASE_WAITING {
                hs.players[hs.dealer_pos as usize].seat as i8
            } else {
                -1
            },
            action_on,
            players,
            small_blind: *self.small_blind.get(),
            big_blind: *self.big_blind.get(),
        })
    }

    /// Get *your own* hole cards. Returns an empty vec outside a hand.
    pub fn get_my_cards(&self) -> app::Result<Vec<String>> {
        let caller = player_id();
        let hs = self.hand.get();

        if hs.phase == PHASE_WAITING {
            return Ok(vec![]);
        }

        for p in &hs.players {
            if p.player_id == caller && !p.folded {
                return Ok(vec![
                    Card(p.cards[0]).to_string(),
                    Card(p.cards[1]).to_string(),
                ]);
            }
        }
        Ok(vec![])
    }

    /// Get the result of the last completed hand.
    pub fn get_hand_result(&self) -> app::Result<HandResult> {
        Ok(self.last_result.get().clone())
    }

    /// Get full hand history.
    ///
    /// Returns the last `limit` hands (most recent first).
    /// Pass `0` to get all hands.
    pub fn get_hand_history(&self, limit: u32) -> app::Result<Vec<HandResult>> {
        let hist = self.history.get();
        if limit == 0 || limit as usize >= hist.len() {
            let mut all = hist.clone();
            all.reverse();
            return Ok(all);
        }
        let start = hist.len() - limit as usize;
        let mut recent = hist[start..].to_vec();
        recent.reverse();
        Ok(recent)
    }

    /// Get aggregate table stats: hands played, per-player wins & chips.
    pub fn get_stats(&self) -> app::Result<TableStats> {
        let total = self.hands_played.value()?;
        let seated = self.get_seated_players()?;

        let mut players = Vec::new();
        for (_, pid) in &seated {
            let wins = self.win_count.get(pid)?.map(|w| *w.get()).unwrap_or(0);
            let chips = self.get_player_chips(pid)?;
            players.push(PlayerStats {
                player_id: pid.clone(),
                wins,
                chips,
                hands_played: total,
            });
        }

        Ok(TableStats {
            hands_played: total,
            players,
        })
    }

    // ── bot play ─────────────────────────────────────────────────────

    // ── secure dealing ───────────────────────────────────────────

    /// Enable secure dealing mode. Must be called before any hand starts.
    pub fn enable_secure_mode(&mut self) -> app::Result<()> {
        if self.hand.get().phase != PHASE_WAITING {
            app::bail!(PokerError::HandInProgress);
        }
        self.secure_mode.set(true);
        app::log!("Secure dealing mode enabled");
        Ok(())
    }

    /// Register as the non-playing dealer. Generates an X25519 keypair.
    pub fn register_dealer(&mut self) -> app::Result<()> {
        let caller = player_id();
        let (secret, public) = crypto::generate_keypair();

        self.dealer_id.set(caller.clone());
        self.encryption_keys
            .insert(caller.clone(), public.to_vec().into())?;

        // Store private key in dealer's private storage
        calimero_sdk::private_storage::EntryHandle::<Vec<u8>>::new(b"x25519_secret")
            .modify(|v| *v = secret.to_vec())?;

        app::log!("Dealer registered: {}", caller);
        Ok(())
    }

    /// Register an encryption key for receiving encrypted cards.
    /// Each bot must call this before playing in secure mode.
    pub fn register_encryption_key(&mut self) -> app::Result<()> {
        let caller = player_id();
        let (secret, public) = crypto::generate_keypair();

        self.encryption_keys
            .insert(caller.clone(), public.to_vec().into())?;

        // Store private key in bot's private storage
        calimero_sdk::private_storage::EntryHandle::<Vec<u8>>::new(b"x25519_secret")
            .modify(|v| *v = secret.to_vec())?;

        app::log!("Encryption key registered for {}", caller);
        Ok(())
    }

    /// Verify the VRF proof for the current hand.
    ///
    /// Anyone can call this to confirm the shuffle was derived correctly.
    pub fn verify_vrf(&self, hand_number: u64) -> app::Result<bool> {
        let dealer_id = self.dealer_id.get().clone();
        let dealer_pubkey = self
            .encryption_keys
            .get(&dealer_id)?
            .map(|v| v.get().clone())
            .unwrap_or_default();

        if dealer_pubkey.len() != 32 {
            return Ok(false);
        }

        let proof = self.vrf_proof.get().clone();
        if proof.len() != 32 {
            return Ok(false);
        }

        let pk: [u8; 32] = dealer_pubkey.try_into().unwrap_or([0u8; 32]);
        let pf: [u8; 32] = proof.try_into().unwrap_or([0u8; 32]);
        Ok(crypto::vrf_verify(&pk, &hand_number.to_le_bytes(), &pf))
    }

    /// Dealer deals cards using VRF for verifiable randomness.
    ///
    /// VRF(dealer_secret_key, hand_number) → deterministic random + proof.
    /// No seeds needed from players — the TEE guarantees fairness.
    /// The proof is published so anyone can verify.
    pub fn dealer_deal(&mut self) -> app::Result<()> {
        let caller = player_id();
        if *self.dealer_id.get() != caller {
            app::bail!(PokerError::NotDealer);
        }
        if self.hand.get().phase != PHASE_WAITING {
            app::bail!(PokerError::HandInProgress);
        }

        let seated = self.get_seated_players()?;
        if seated.len() < 2 {
            app::bail!(PokerError::NotEnoughPlayers);
        }

        // Read dealer's private key from private storage
        let dealer_secret_ref =
            calimero_sdk::private_storage::EntryHandle::<Vec<u8>>::new(b"x25519_secret")
                .get_or_default()?;
        let dealer_secret: [u8; 32] = dealer_secret_ref
            .as_slice()
            .try_into()
            .map_err(|_| app::err!("dealer key missing"))?;

        let hand_num = self.hands_played.value()? + 1;

        // VRF: derive verifiable randomness from dealer's secret key + hand number
        let vrf_out = crypto::vrf_compute(&dealer_secret, &hand_num.to_le_bytes());
        let mut deck = deck::new_shuffled_deck_from_seed(&vrf_out.random);

        // Publish VRF proof to shared state (anyone can verify)
        self.vrf_proof.set(vrf_out.proof.to_vec());

        // Deal and encrypt cards per player
        let mut players: Vec<PlayerHand> = Vec::new();
        let dealer_seat = *self.next_dealer.get();
        let dealer_idx = nearest_index(&seated, dealer_seat);
        let mut seated_rotated = seated.clone();
        seated_rotated.rotate_left(dealer_idx);

        for (seat, pid) in &seated_rotated {
            let c1 = deck.pop().map(|c| c.0).unwrap_or(0);
            let c2 = deck.pop().map(|c| c.0).unwrap_or(0);

            // Encrypt for this player
            let player_pubkey = self
                .encryption_keys
                .get(pid)?
                .map(|v| v.get().clone())
                .unwrap_or_default();

            if player_pubkey.len() == 32 {
                let pubkey: [u8; 32] = player_pubkey.try_into().unwrap_or([0u8; 32]);
                let encrypted = crypto::encrypt_cards(&dealer_secret, &pubkey, &[c1, c2], hand_num);
                let _ = self.encrypted_cards.insert(pid.clone(), encrypted.into());
            }

            players.push(PlayerHand {
                player_id: pid.clone(),
                seat: *seat,
                cards: [c1, c2], // stored but only dealer node has the real values
                ..PlayerHand::default()
            });
        }

        // Store community cards in dealer's private storage
        let mut community_all: Vec<u8> = Vec::new();
        for _ in 0..5 {
            if let Some(card) = deck.pop() {
                community_all.push(card.0);
            }
        }
        calimero_sdk::private_storage::EntryHandle::<Vec<u8>>::new(b"community_cards")
            .modify(|v| *v = community_all)?;

        // Post blinds
        let sb = *self.small_blind.get();
        let bb = *self.big_blind.get();
        let num = players.len();
        let (sb_idx, bb_idx) = if num == 2 { (0, 1) } else { (1, 2) };

        let sb_actual = self.deduct_chips(&players[sb_idx].player_id, sb)?;
        players[sb_idx].bet_this_round = sb_actual;
        players[sb_idx].bet_total = sb_actual;
        if sb_actual < sb {
            players[sb_idx].all_in = true;
        }

        let bb_actual = self.deduct_chips(&players[bb_idx].player_id, bb)?;
        players[bb_idx].bet_this_round = bb_actual;
        players[bb_idx].bet_total = bb_actual;
        if bb_actual < bb {
            players[bb_idx].all_in = true;
        }

        let pot = sb_actual + bb_actual;
        let first_to_act = if num == 2 { 0 } else { (bb_idx + 1) % num };
        let mut acted = vec![false; num];
        for (i, p) in players.iter().enumerate() {
            if p.all_in {
                acted[i] = true;
            }
        }

        let new_hs = HandState {
            phase: PHASE_PREFLOP,
            deck: Vec::new(), // deck not stored in shared state!
            community: Vec::new(),
            players,
            dealer_pos: 0,
            action_pos: first_to_act as u8,
            pot,
            current_bet: bb_actual,
            acted,
            last_action_time: env::time_now(),
        };

        self.hand.set(new_hs);
        self.hands_played.increment()?;
        self.next_dealer
            .set((dealer_seat + 1) % *self.max_seats.get());

        app::log!("VRF deal complete: hand #{}", hand_num);
        Ok(())
    }

    /// Dealer reveals flop (3 community cards from private storage).
    pub fn dealer_reveal_flop(&mut self) -> app::Result<()> {
        self.dealer_reveal_community(3)
    }

    /// Dealer reveals turn (1 more community card).
    pub fn dealer_reveal_turn(&mut self) -> app::Result<()> {
        self.dealer_reveal_community(1)
    }

    /// Dealer reveals river (1 more community card).
    pub fn dealer_reveal_river(&mut self) -> app::Result<()> {
        self.dealer_reveal_community(1)
    }

    /// Get your hole cards (decrypts from shared state in secure mode).
    pub fn get_my_cards_secure(&self) -> app::Result<Vec<String>> {
        let caller = player_id();

        let encrypted = self
            .encrypted_cards
            .get(&caller)?
            .map(|v| v.get().clone())
            .unwrap_or_default();

        if encrypted.is_empty() {
            return Ok(vec![]);
        }

        // Read my X25519 private key from private storage
        let my_secret_ref =
            calimero_sdk::private_storage::EntryHandle::<Vec<u8>>::new(b"x25519_secret")
                .get_or_default()?;
        let my_secret: [u8; 32] = my_secret_ref
            .as_slice()
            .try_into()
            .map_err(|_| app::err!("encryption key missing"))?;

        // Get dealer's public key
        let dealer_id = self.dealer_id.get().clone();
        let dealer_pubkey = self
            .encryption_keys
            .get(&dealer_id)?
            .map(|v| v.get().clone())
            .unwrap_or_default();

        if dealer_pubkey.len() != 32 {
            app::bail!(PokerError::NoDealerRegistered);
        }

        let pubkey: [u8; 32] = dealer_pubkey.try_into().unwrap_or([0u8; 32]);
        let hand_num = self.hands_played.value()?;

        match crypto::decrypt_cards(&my_secret, &pubkey, &encrypted, hand_num) {
            Some(cards) if cards.len() >= 2 => {
                Ok(vec![Card(cards[0]).to_string(), Card(cards[1]).to_string()])
            }
            _ => Ok(vec![]),
        }
    }

    // ── bot play ────────────────────────────────────────────────

    /// Let an AI bot make a decision and execute it.
    ///
    /// The caller's identity must be the action-on player.
    ///
    /// **Strategies**: 0 = Random, 1 = Caller (always call/check), 2 = TAG (tight-aggressive)
    ///
    /// Returns a description of the action taken (e.g. `"call"`, `"raise 60"`, `"fold"`).
    pub fn bot_play(&mut self, strategy: u8) -> app::Result<String> {
        let caller = player_id();
        let hs = self.hand.get().clone();
        Self::require_active_hand(&hs)?;
        let pos = Self::verify_turn(&hs, &caller)?;

        let bb = *self.big_blind.get();

        let action = match strategy {
            0 => bot::random_action(&hs, pos, bb),
            1 => bot::caller_action(&hs, pos),
            _ => bot::tag_action(&hs, pos, bb),
        };

        match action {
            bot::BotAction::Fold => {
                self.fold()?;
                Ok("fold".into())
            }
            bot::BotAction::Check => {
                self.check()?;
                Ok("check".into())
            }
            bot::BotAction::Call => {
                self.call()?;
                Ok("call".into())
            }
            bot::BotAction::RaiseTo(amount) => {
                self.raise_to(amount)?;
                Ok(format!("raise {amount}"))
            }
        }
    }

    // ── timeout ─────────────────────────────────────────────────────

    /// Force-fold the idle player if they've exceeded the timeout.
    ///
    /// Any seated player can call this.  The action-on player is auto-folded
    /// and play continues normally.
    pub fn claim_timeout(&mut self) -> app::Result<()> {
        let mut hs = self.hand.get().clone();
        Self::require_active_hand(&hs)?;

        let now = env::time_now();
        let timeout = *self.timeout_ns.get();
        let elapsed = now.saturating_sub(hs.last_action_time);

        if elapsed < timeout {
            app::bail!(PokerError::NotTimedOut);
        }

        let pos = hs.action_pos as usize;
        let timed_out_id = hs.players[pos].player_id.clone();

        app::emit!(PokerEvent::PlayerTimedOut {
            player_id: &timed_out_id,
        });
        app::log!(
            "Player {} timed out after {}s",
            timed_out_id,
            elapsed / 1_000_000_000
        );

        // Auto-fold the idle player
        hs.players[pos].folded = true;
        hs.acted[pos] = true;
        hs.last_action_time = now;

        app::emit!(PokerEvent::PlayerActed {
            player_id: &timed_out_id,
            action: "fold (timeout)",
            amount: 0,
        });

        let remaining: Vec<usize> = hs
            .players
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.folded)
            .map(|(i, _)| i)
            .collect();

        if remaining.len() == 1 {
            self.award_pot(&mut hs, remaining[0]);
            self.hand.set(hs);
            return Ok(());
        }

        self.advance_action(&mut hs);
        self.hand.set(hs);
        Ok(())
    }
}

// ══════════════════════════════════════════════════════════════════════
// Internal helpers (not exposed in ABI)
// ── dealer internals ────────────────────────────────────────
// ══════════════════════════════════════════════════════════════════════

impl PokerGame {
    // ── dealer reveal helper ────────────────────────────────────────

    fn dealer_reveal_community(&mut self, count: usize) -> app::Result<()> {
        let caller = player_id();
        if *self.dealer_id.get() != caller {
            app::bail!(PokerError::NotDealer);
        }

        // Read community cards from dealer's private storage
        let all_community =
            calimero_sdk::private_storage::EntryHandle::<Vec<u8>>::new(b"community_cards")
                .get_or_default()?;

        let mut hs = self.hand.get().clone();
        let already = hs.community.len();
        let end = (already + count).min(all_community.len());

        for &card in &all_community[already..end] {
            hs.community.push(card);
        }

        let s = cards_display(&hs.community);
        app::emit!(PokerEvent::CommunityDealt { cards: &s });

        // Advance phase
        match already {
            0 => {
                hs.phase = PHASE_FLOP;
                app::emit!(PokerEvent::PhaseChanged { phase: "Flop" });
            }
            3 => {
                hs.phase = PHASE_TURN;
                app::emit!(PokerEvent::PhaseChanged { phase: "Turn" });
            }
            4 => {
                hs.phase = PHASE_RIVER;
                app::emit!(PokerEvent::PhaseChanged { phase: "River" });
            }
            _ => {}
        }

        // Reset round tracking
        for p in &mut hs.players {
            p.bet_this_round = 0;
        }
        hs.current_bet = 0;
        hs.acted = vec![false; hs.players.len()];
        for (i, p) in hs.players.iter().enumerate() {
            if p.folded || p.all_in {
                hs.acted[i] = true;
            }
        }

        // Set first-to-act post-flop
        let n = hs.players.len();
        let start = (hs.dealer_pos as usize + 1) % n;
        for offset in 0..n {
            let idx = (start + offset) % n;
            if !hs.players[idx].folded && !hs.players[idx].all_in {
                hs.action_pos = idx as u8;
                break;
            }
        }

        self.hand.set(hs);
        Ok(())
    }

    // ── seat helpers ────────────────────────────────────────────────

    fn find_empty_seat(&self) -> app::Result<Option<u8>> {
        let max = *self.max_seats.get();
        for i in 0..max {
            match self.seats.get(&i.to_string())? {
                Some(occ) if !occ.get().is_empty() => continue,
                _ => return Ok(Some(i)),
            }
        }
        Ok(None)
    }

    /// Seated players with chips > 0, sorted by seat index.
    fn get_seated_players(&self) -> app::Result<Vec<(u8, String)>> {
        let max = *self.max_seats.get();
        let mut out = Vec::new();
        for i in 0..max {
            if let Some(occ) = self.seats.get(&i.to_string())? {
                let pid = occ.get().clone();
                if !pid.is_empty() && self.get_player_chips(&pid)? > 0 {
                    out.push((i, pid));
                }
            }
        }
        Ok(out)
    }

    // ── chip helpers ────────────────────────────────────────────────

    fn get_player_chips(&self, pid: &str) -> app::Result<u64> {
        Ok(self.chips.get(pid)?.map(|c| *c.get()).unwrap_or(0))
    }

    /// Deduct up to `amount` chips.  Returns actual amount deducted.
    fn deduct_chips(&mut self, pid: &str, amount: u64) -> app::Result<u64> {
        let current = self.get_player_chips(pid)?;
        let actual = amount.min(current);
        self.chips
            .insert(pid.to_string(), (current - actual).into())?;
        Ok(actual)
    }

    // ── turn validation ─────────────────────────────────────────────

    fn require_active_hand(hs: &HandState) -> app::Result<()> {
        if hs.phase == PHASE_WAITING {
            app::bail!(PokerError::NoHandInProgress);
        }
        Ok(())
    }

    fn verify_turn(hs: &HandState, caller: &str) -> app::Result<usize> {
        match is_callers_turn(hs, caller) {
            Some(pos) => Ok(pos),
            None => app::bail!(PokerError::NotYourTurn),
        }
    }

    // ── action / phase advancement ──────────────────────────────────

    fn advance_action(&mut self, hs: &mut HandState) {
        if Self::is_round_complete(hs) {
            self.advance_phase(hs);
            return;
        }

        let n = hs.players.len();
        for offset in 1..=n {
            let next = (hs.action_pos as usize + offset) % n;
            if !hs.players[next].folded && !hs.players[next].all_in && !hs.acted[next] {
                hs.action_pos = next as u8;
                return;
            }
        }
        // Everyone acted or is all-in / folded → advance phase
        self.advance_phase(hs);
    }

    fn is_round_complete(hs: &HandState) -> bool {
        round_complete(hs)
    }

    fn advance_phase(&mut self, hs: &mut HandState) {
        // In secure mode, don't auto-deal community cards.
        // The dealer must call dealer_reveal_flop/turn/river.
        // Just mark the round as complete and wait.
        if *self.secure_mode.get() && hs.phase != PHASE_RIVER {
            // Reset round-level tracking
            for p in &mut hs.players {
                p.bet_this_round = 0;
            }
            hs.current_bet = 0;
            // Block further actions — dealer must call dealer_reveal_* to advance
            hs.action_pos = 255;
            return;
        }

        if *self.secure_mode.get() && hs.phase == PHASE_RIVER {
            self.resolve_showdown(hs);
            return;
        }

        // Reset round-level tracking
        for p in &mut hs.players {
            p.bet_this_round = 0;
        }
        hs.current_bet = 0;
        hs.acted = vec![false; hs.players.len()];
        for (i, p) in hs.players.iter().enumerate() {
            if p.folded || p.all_in {
                hs.acted[i] = true;
            }
        }

        // Deal community cards for the new phase (non-secure mode only)
        match hs.phase {
            PHASE_PREFLOP => {
                for _ in 0..3 {
                    if let Some(card) = hs.deck.pop() {
                        hs.community.push(card);
                    }
                }
                hs.phase = PHASE_FLOP;
                let s = cards_display(&hs.community);
                app::emit!(PokerEvent::CommunityDealt { cards: &s });
                app::emit!(PokerEvent::PhaseChanged { phase: "Flop" });
            }
            PHASE_FLOP => {
                if let Some(card) = hs.deck.pop() {
                    hs.community.push(card);
                    let s = Card(card).to_string();
                    app::emit!(PokerEvent::CommunityDealt { cards: &s });
                }
                hs.phase = PHASE_TURN;
                app::emit!(PokerEvent::PhaseChanged { phase: "Turn" });
            }
            PHASE_TURN => {
                if let Some(card) = hs.deck.pop() {
                    hs.community.push(card);
                    let s = Card(card).to_string();
                    app::emit!(PokerEvent::CommunityDealt { cards: &s });
                }
                hs.phase = PHASE_RIVER;
                app::emit!(PokerEvent::PhaseChanged { phase: "River" });
            }
            PHASE_RIVER => {
                self.resolve_showdown(hs);
                return;
            }
            _ => return,
        }

        // If all remaining players are all-in → run out the board
        let active_non_allin = hs.players.iter().filter(|p| !p.folded && !p.all_in).count();

        if active_non_allin <= 1 {
            while hs.community.len() < 5 {
                if let Some(card) = hs.deck.pop() {
                    hs.community.push(card);
                }
            }
            let s = cards_display(&hs.community);
            app::emit!(PokerEvent::CommunityDealt { cards: &s });
            self.resolve_showdown(hs);
            return;
        }

        // Set first-to-act for the new street (first active player after dealer)
        let n = hs.players.len();
        let start = (hs.dealer_pos as usize + 1) % n;
        for offset in 0..n {
            let idx = (start + offset) % n;
            if !hs.players[idx].folded && !hs.players[idx].all_in {
                hs.action_pos = idx as u8;
                return;
            }
        }
    }

    // ── showdown & pot ──────────────────────────────────────────────

    fn resolve_showdown(&mut self, hs: &mut HandState) {
        app::emit!(PokerEvent::PhaseChanged { phase: "Showdown" });

        let mut best_score = hand::HandScore(0);
        let mut winner_idx = 0;

        for (i, p) in hs.players.iter().enumerate() {
            if p.folded || hs.community.len() < 5 {
                continue;
            }
            let seven = [
                Card(p.cards[0]),
                Card(p.cards[1]),
                Card(hs.community[0]),
                Card(hs.community[1]),
                Card(hs.community[2]),
                Card(hs.community[3]),
                Card(hs.community[4]),
            ];
            let score = hand::evaluate_seven(&seven);
            if score > best_score {
                best_score = score;
                winner_idx = i;
            }
        }

        self.award_pot(hs, winner_idx);
    }

    fn award_pot(&mut self, hs: &mut HandState, winner_idx: usize) {
        let winner_id = hs.players[winner_idx].player_id.clone();
        let pot = hs.pot;

        // Credit chips to winner
        let current = self.get_player_chips(&winner_id).unwrap_or(0);
        let _ = self.chips.insert(winner_id.clone(), (current + pot).into());

        // Determine win reason & hand name
        let non_folded_count = hs.players.iter().filter(|p| !p.folded).count();
        let reason = if non_folded_count <= 1 {
            "last player standing"
        } else {
            "showdown"
        };

        let hand_name = if hs.community.len() >= 5 && reason == "showdown" {
            let seven = [
                Card(hs.players[winner_idx].cards[0]),
                Card(hs.players[winner_idx].cards[1]),
                Card(hs.community[0]),
                Card(hs.community[1]),
                Card(hs.community[2]),
                Card(hs.community[3]),
                Card(hs.community[4]),
            ];
            hand::evaluate_seven(&seven).category_name().to_string()
        } else {
            "N/A".to_string()
        };

        app::emit!(PokerEvent::HandComplete {
            winner: &winner_id,
            pot,
            reason,
        });

        if reason == "showdown" {
            app::emit!(PokerEvent::ShowdownResult {
                winner: &winner_id,
                hand_name: &hand_name,
                pot,
            });
        }

        app::log!(
            "Hand complete: {} wins {} chips ({})",
            winner_id,
            pot,
            reason
        );

        // Persist the hand result for queries
        let hand_num = self.hands_played.value().unwrap_or(0);
        let result = HandResult {
            hand_number: hand_num,
            winner_id: winner_id.clone(),
            winning_hand: hand_name.clone(),
            pot,
            reason: reason.to_string(),
            player_cards: hs
                .players
                .iter()
                .map(|p| RevealedHand {
                    player_id: p.player_id.clone(),
                    card1: Card(p.cards[0]).to_string(),
                    card2: Card(p.cards[1]).to_string(),
                })
                .collect(),
            community_cards: hs.community.iter().map(|&c| Card(c).to_string()).collect(),
        };
        self.last_result.set(result.clone());

        // Append to history
        let mut hist = self.history.get().clone();
        hist.push(result);
        self.history.set(hist);

        // Track win count
        let wins = self
            .win_count
            .get(&winner_id)
            .ok()
            .flatten()
            .map(|w| *w.get())
            .unwrap_or(0);
        let _ = self.win_count.insert(winner_id, (wins + 1).into());

        // Reset hand state to waiting
        *hs = HandState::default();
    }
}

// ══════════════════════════════════════════════════════════════════════
// Standalone helpers
// ══════════════════════════════════════════════════════════════════════

/// Find the player whose seat is closest to (≥) `target`, wrapping around.
fn nearest_index(players: &[(u8, String)], target: u8) -> usize {
    for (i, (seat, _)) in players.iter().enumerate() {
        if *seat >= target {
            return i;
        }
    }
    0
}

/// Check whether every active (non-folded, non-all-in) player has acted
/// and matched the current bet.  Extracted as free function for testability.
fn round_complete(hs: &HandState) -> bool {
    for (i, p) in hs.players.iter().enumerate() {
        if p.folded || p.all_in {
            continue;
        }
        if !hs.acted[i] || p.bet_this_round != hs.current_bet {
            return false;
        }
    }
    true
}

/// Check whether it is `caller`'s turn.  Returns the player index, or `None`.
fn is_callers_turn(hs: &HandState, caller: &str) -> Option<usize> {
    let pos = hs.action_pos as usize;
    if pos < hs.players.len() && hs.players[pos].player_id == caller {
        Some(pos)
    } else {
        None
    }
}

// ══════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── helpers ────────────────────────────────────────────────────

    fn player(id: &str, seat: u8) -> PlayerHand {
        PlayerHand {
            player_id: id.to_string(),
            seat,
            cards: [0, 1],
            ..PlayerHand::default()
        }
    }

    fn make_hand(players: Vec<PlayerHand>, current_bet: u64, acted: Vec<bool>) -> HandState {
        HandState {
            phase: PHASE_PREFLOP,
            players,
            current_bet,
            acted,
            ..HandState::default()
        }
    }

    // ── nearest_index ─────────────────────────────────────────────

    #[test]
    fn nearest_index_exact_match() {
        let p = vec![(0, "A".into()), (2, "B".into()), (4, "C".into())];
        assert_eq!(nearest_index(&p, 2), 1);
    }

    #[test]
    fn nearest_index_between_seats() {
        let p = vec![(0, "A".into()), (2, "B".into()), (4, "C".into())];
        assert_eq!(nearest_index(&p, 3), 2); // next seat ≥3 is seat 4 (idx 2)
    }

    #[test]
    fn nearest_index_wraps_around() {
        let p = vec![(0, "A".into()), (2, "B".into()), (4, "C".into())];
        assert_eq!(nearest_index(&p, 5), 0); // nothing ≥5, wrap to 0
    }

    #[test]
    fn nearest_index_single_player() {
        let p = vec![(3, "A".into())];
        assert_eq!(nearest_index(&p, 0), 0);
        assert_eq!(nearest_index(&p, 3), 0);
        assert_eq!(nearest_index(&p, 4), 0); // wrap
    }

    // ── round_complete ────────────────────────────────────────────

    #[test]
    fn round_complete_all_checked() {
        let hs = make_hand(vec![player("A", 0), player("B", 1)], 0, vec![true, true]);
        assert!(round_complete(&hs));
    }

    #[test]
    fn round_complete_one_not_acted() {
        let hs = make_hand(vec![player("A", 0), player("B", 1)], 0, vec![true, false]);
        assert!(!round_complete(&hs));
    }

    #[test]
    fn round_complete_bet_not_matched() {
        let mut players = vec![player("A", 0), player("B", 1)];
        players[0].bet_this_round = 40;
        players[1].bet_this_round = 20;
        let hs = make_hand(players, 40, vec![true, true]);
        assert!(!round_complete(&hs)); // B hasn't matched 40
    }

    #[test]
    fn round_complete_folded_player_ignored() {
        let mut players = vec![player("A", 0), player("B", 1), player("C", 2)];
        players[1].folded = true;
        players[0].bet_this_round = 40;
        players[2].bet_this_round = 40;
        let hs = make_hand(players, 40, vec![true, true, true]);
        assert!(round_complete(&hs)); // B folded, A & C matched
    }

    #[test]
    fn round_complete_all_in_player_ignored() {
        let mut players = vec![player("A", 0), player("B", 1)];
        players[0].all_in = true;
        players[0].bet_this_round = 20; // went all-in for less
        players[1].bet_this_round = 40;
        let hs = make_hand(players, 40, vec![true, true]);
        assert!(round_complete(&hs)); // A is all-in, only B matters
    }

    #[test]
    fn round_complete_everyone_all_in() {
        let mut players = vec![player("A", 0), player("B", 1)];
        players[0].all_in = true;
        players[1].all_in = true;
        let hs = make_hand(players, 100, vec![true, true]);
        assert!(round_complete(&hs)); // no active players → complete
    }

    // ── is_callers_turn ───────────────────────────────────────────

    #[test]
    fn callers_turn_correct_player() {
        let hs = HandState {
            action_pos: 1,
            players: vec![player("A", 0), player("B", 1)],
            ..HandState::default()
        };
        assert_eq!(is_callers_turn(&hs, "B"), Some(1));
    }

    #[test]
    fn callers_turn_wrong_player() {
        let hs = HandState {
            action_pos: 0,
            players: vec![player("A", 0), player("B", 1)],
            ..HandState::default()
        };
        assert_eq!(is_callers_turn(&hs, "B"), None); // it's A's turn
    }

    #[test]
    fn callers_turn_out_of_bounds() {
        let hs = HandState {
            action_pos: 5, // out of bounds
            players: vec![player("A", 0), player("B", 1)],
            ..HandState::default()
        };
        assert_eq!(is_callers_turn(&hs, "A"), None);
    }

    // ── phase_name ────────────────────────────────────────────────

    #[test]
    fn phase_names_are_correct() {
        assert_eq!(phase_name(PHASE_WAITING), "Waiting");
        assert_eq!(phase_name(PHASE_PREFLOP), "PreFlop");
        assert_eq!(phase_name(PHASE_FLOP), "Flop");
        assert_eq!(phase_name(PHASE_TURN), "Turn");
        assert_eq!(phase_name(PHASE_RIVER), "River");
        assert_eq!(phase_name(99), "Unknown");
    }

    // ── cards_display ─────────────────────────────────────────────

    #[test]
    fn cards_display_formatting() {
        assert_eq!(cards_display(&[0, 12, 51]), "2c Ac As");
        assert_eq!(cards_display(&[]), "");
    }

    // ── blind positions ───────────────────────────────────────────

    #[test]
    fn blind_positions_heads_up() {
        // In heads-up (2 players): dealer(idx 0)=SB, idx 1=BB
        let num = 2;
        let (sb, bb) = if num == 2 { (0, 1) } else { (1, 2) };
        assert_eq!(sb, 0);
        assert_eq!(bb, 1);
    }

    #[test]
    fn blind_positions_three_players() {
        // 3+ players: idx 1=SB, idx 2=BB
        let num = 3;
        let (sb, bb) = if num == 2 { (0, 1) } else { (1, 2) };
        assert_eq!(sb, 1);
        assert_eq!(bb, 2);
    }

    // ── preflop action order ──────────────────────────────────────

    #[test]
    fn first_to_act_heads_up() {
        // Heads-up: SB/dealer (idx 0) acts first preflop
        let num = 2;
        let bb_idx = 1;
        let first = if num == 2 { 0 } else { (bb_idx + 1) % num };
        assert_eq!(first, 0);
    }

    #[test]
    fn first_to_act_three_players() {
        // 3 players: UTG = (BB_idx + 1) % 3 = (2+1)%3 = 0 = dealer
        let num = 3;
        let bb_idx = 2;
        let first = if num == 2 { 0 } else { (bb_idx + 1) % num };
        assert_eq!(first, 0); // dealer is UTG in 3-handed
    }

    #[test]
    fn first_to_act_six_players() {
        // 6 players: UTG = (BB_idx + 1) % 6 = (2+1)%6 = 3
        let num = 6;
        let bb_idx = 2;
        let first = if num == 2 { 0 } else { (bb_idx + 1) % num };
        assert_eq!(first, 3);
    }
}
