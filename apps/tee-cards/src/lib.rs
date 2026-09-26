//! Hidden cards no player can peek at or stack: TEE authorship with secrets.
//!
//! Three kinds of state, three audiences:
//!
//! - **Seats** — each player's device key, written by the player
//!   ([`UserStorage`], one slot per account, so nobody can repoint another
//!   player's seat at a key of their own).
//! - **The deck** — the undealt cards, a [`TeeSecret`]: sealed to every TEE
//!   authority, so members replicate it and cannot read it.
//! - **Hands** — each card sealed to its player's seat key, in `TeeOnly` state:
//!   every member stores it, only the player's own node opens it.
//!
//! A player asks for a card with [`TeeCards::draw`]. The elected TEE authority
//! runs [`TeeCards::deal`] inside the enclave: it opens the deck, takes the top
//! card, seals it to the player and seals the rest back. A timer,
//! [`TeeCards::reshuffle`], shuffles a fresh deck whenever the current one is
//! missing or spent, with no member asking.

use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env, AccountId};
use calimero_storage::collections::{
    LwwRegister, Sealed, TeeOnly, TeeSecret, UnorderedMap, UserStorage,
};
use thiserror::Error;

/// Cards in a fresh deck: `0..52`, suit-major (`card / 13` is the suit).
const DECK_SIZE: u8 = 52;

#[app::state(emits = for<'a> Event<'a>)]
pub struct TeeCards {
    /// Account → the device key its cards are sealed to.
    seats: UserStorage<LwwRegister<[u8; 32]>>,
    /// The undealt cards, in dealing order. Only the TEE can read it.
    deck: TeeSecret<Vec<u8>>,
    /// Player (account, hex) → their cards, each sealed to their seat key.
    hands: TeeOnly<UnorderedMap<String, LwwRegister<Vec<Sealed<u8>>>>>,
    /// How many cards the deck still holds. Public; written only by the TEE.
    remaining: TeeOnly<LwwRegister<u32>>,
}

#[app::event]
pub enum Event<'a> {
    /// A seated player asked for a card; the TEE handler deals it.
    CardRequested { player: &'a str },
    /// The TEE dealt a card to `player`. Only they can see which.
    CardDealt { player: &'a str },
    /// The TEE shuffled a fresh deck.
    Shuffled,
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error {
    #[error("take a seat before drawing")]
    NotSeated,
}

#[app::logic]
impl TeeCards {
    #[app::init]
    pub fn init() -> TeeCards {
        TeeCards {
            seats: UserStorage::new(),
            deck: TeeSecret::new_tee_secret(),
            hands: TeeOnly::new_tee_only(),
            remaining: TeeOnly::new_tee_only(),
        }
    }

    /// Take a seat: cards dealt to you are sealed to the device you call this
    /// from, and only that device can read them.
    pub fn sit(&mut self) -> app::Result<()> {
        let _previous = self.seats.insert(LwwRegister::new(env::device_id()))?;
        Ok(())
    }

    /// Ask the TEE for a card.
    pub fn draw(&mut self) -> app::Result<()> {
        if !self.seats.contains_current_user()? {
            app::bail!(Error::NotSeated);
        }
        let player = hex(&env::account_id());
        app::emit!((Event::CardRequested { player: &player }, "tee:deal"));
        Ok(())
    }

    /// Deal the top card to `player`. Runs only on the elected TEE authority.
    ///
    /// A player who has left their seat, or never took one, gets nothing.
    #[app::tee]
    pub fn deal(&mut self, player: String) -> app::Result<()> {
        let Some(account) = parse_hex(&player) else {
            return Ok(());
        };
        let Some(seat) = self.seats.get_for_user(&AccountId::from(account))? else {
            return Ok(());
        };
        let mut deck = match self.deck.reveal()? {
            Some(deck) if !deck.is_empty() => deck,
            _ => {
                app::emit!(Event::Shuffled);
                shuffled_deck()
            }
        };
        let Some(card) = deck.pop() else {
            return Ok(());
        };
        let sealed = Sealed::to_key(seat.get(), &card)?;

        self.deck.set(&deck)?;
        let _previous = self.remaining.insert(LwwRegister::new(
            u32::try_from(deck.len()).unwrap_or(u32::MAX),
        ))?;
        let hands = self.hands.get_mut()?;
        let mut hand = hands
            .get(&player)?
            .map(|hand| hand.get().clone())
            .unwrap_or_default();
        hand.push(sealed);
        let _previous = hands.insert(player.clone(), LwwRegister::new(hand))?;
        app::emit!(Event::CardDealt { player: &player });
        Ok(())
    }

    /// Shuffle a fresh deck when there is none or it is spent. Fired by the
    /// node's TEE scheduler once a minute, with no member asking; a tick with
    /// cards still in the deck writes nothing.
    #[app::tee(every = "1m")]
    pub fn reshuffle(&mut self) -> app::Result<()> {
        if self.deck.reveal()?.is_some_and(|deck| !deck.is_empty()) {
            return Ok(());
        }
        let deck = shuffled_deck();
        self.deck.set(&deck)?;
        let _previous = self
            .remaining
            .insert(LwwRegister::new(u32::from(DECK_SIZE)))?;
        app::emit!(Event::Shuffled);
        Ok(())
    }

    /// Your hand, read on your own node. Cards sealed to another device of
    /// yours do not open here.
    pub fn my_hand(&self) -> app::Result<Vec<u8>> {
        let Some(hands) = self.hands.try_get()? else {
            return Ok(Vec::new());
        };
        let Some(hand) = hands.get(&hex(&env::account_id()))? else {
            return Ok(Vec::new());
        };
        Ok(hand.get().iter().filter_map(Sealed::open).collect())
    }

    /// How many cards `player` holds. Anyone may ask; nobody but `player` can
    /// see which.
    pub fn hand_size(&self, player: String) -> app::Result<u32> {
        let Some(hands) = self.hands.try_get()? else {
            return Ok(0);
        };
        Ok(hands.get(&player)?.map_or(0, |hand| {
            u32::try_from(hand.get().len()).unwrap_or(u32::MAX)
        }))
    }

    /// How many cards the deck still holds, or `None` before the first shuffle.
    pub fn cards_left(&self) -> app::Result<Option<u32>> {
        Ok(self.remaining.try_get()?.map(|left| *left.get()))
    }
}

/// A full deck in an order drawn inside the enclave (Fisher–Yates).
fn shuffled_deck() -> Vec<u8> {
    let mut deck: Vec<u8> = (0..DECK_SIZE).collect();
    for i in (1..deck.len()).rev() {
        let j = below(u32::try_from(i + 1).unwrap_or(u32::MAX)) as usize;
        deck.swap(i, j);
    }
    deck
}

/// A uniform draw from `0..bound`, by rejection sampling.
fn below(bound: u32) -> u32 {
    let zone = u32::MAX - (u32::MAX % bound);
    loop {
        let mut bytes = [0u8; 4];
        env::tee_random_bytes(&mut bytes);
        let draw = u32::from_le_bytes(bytes);
        if draw < zone {
            return draw % bound;
        }
    }
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use calimero_sdk::testing::TestHost;

    use super::*;

    const ALICE: [u8; 32] = [0xA1; 32];
    const BOB: [u8; 32] = [0xB0; 32];

    /// Deal every card the TEE handler was asked for.
    fn run_deals(app: &mut TestHost<TeeCards>) {
        for event in app.take_events() {
            if event.handler.as_deref() == Some("tee:deal") {
                let player: String = calimero_sdk::serde_json::from_slice::<
                    calimero_sdk::serde_json::Value,
                >(&event.data)
                .unwrap()["player"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                app.call_as_tee(|s| s.deal(player.clone())).unwrap();
            }
        }
    }

    fn seat_and_draw(app: &mut TestHost<TeeCards>, who: [u8; 32], cards: usize) {
        app.call_as_account(who, who, |s| s.sit()).unwrap();
        for _ in 0..cards {
            app.call_as_account(who, who, |s| s.draw()).unwrap();
            run_deals(app);
        }
    }

    #[test]
    fn each_player_sees_only_their_own_cards() {
        let mut app = TestHost::new(TeeCards::init);
        seat_and_draw(&mut app, ALICE, 3);
        seat_and_draw(&mut app, BOB, 2);

        let alice = app.call_as_account(ALICE, ALICE, |s| s.my_hand()).unwrap();
        let bob = app.call_as_account(BOB, BOB, |s| s.my_hand()).unwrap();
        assert_eq!(alice.len(), 3);
        assert_eq!(bob.len(), 2);
        assert!(alice.iter().chain(&bob).all(|card| *card < DECK_SIZE));
        let mut all: Vec<u8> = alice.iter().chain(&bob).copied().collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 5, "no card is dealt twice from one deck");

        // Bob, on his own device, cannot open Alice's hand even though he
        // replicates it: it is sealed to her seat key.
        let alice_hex = hex(&ALICE);
        assert_eq!(
            app.view(|s| s.hand_size(alice_hex.clone())).unwrap(),
            3,
            "everyone sees how many cards she holds"
        );
        let peek = app.call_as_account(BOB, BOB, |s| {
            let hands = s.hands.try_get().unwrap().unwrap();
            let hand = hands.get(&alice_hex).unwrap().unwrap();
            hand.get().iter().filter_map(Sealed::open).count()
        });
        assert_eq!(peek, 0, "no other player can open her cards");
        assert_eq!(app.view(|s| s.cards_left()).unwrap(), Some(47));
    }

    #[test]
    fn only_the_tee_can_read_the_deck() {
        let mut app = TestHost::new(TeeCards::init);
        app.call_as_tee(|s| s.reshuffle()).unwrap();
        assert_eq!(app.view(|s| s.cards_left()).unwrap(), Some(52));

        let tee_view = app.call_as_tee(|s| s.deck.reveal().unwrap()).unwrap();
        assert_eq!(tee_view.len(), 52);
        let member_view = app.call_as_account(ALICE, ALICE, |s| s.deck.reveal());
        assert!(member_view.is_err(), "the deck does not open for a member");
    }

    #[test]
    fn the_timer_shuffles_once_and_then_stands_by() {
        let mut app = TestHost::new(TeeCards::init);
        app.call_as_tee(|s| s.reshuffle()).unwrap();
        let first = app.call_as_tee(|s| s.deck.reveal().unwrap()).unwrap();
        app.call_as_tee(|s| s.reshuffle()).unwrap();
        let second = app.call_as_tee(|s| s.deck.reveal().unwrap()).unwrap();
        assert_eq!(first, second, "a deck with cards left is not reshuffled");
    }

    #[test]
    fn a_member_cannot_deal() {
        let mut app = TestHost::new(TeeCards::init);
        app.call_as_account(ALICE, ALICE, |s| s.sit()).unwrap();
        let alice_hex = hex(&ALICE);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            app.call_as_account(ALICE, ALICE, |s| s.deal(alice_hex.clone()))
        }));
        assert!(outcome.is_err(), "#[app::tee] must refuse a member");
        assert!(app
            .call_as_account(ALICE, ALICE, |s| s.my_hand())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn drawing_needs_a_seat() {
        let mut app = TestHost::new(TeeCards::init);
        assert!(app.call_as_account(ALICE, ALICE, |s| s.draw()).is_err());
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(parse_hex(&hex(&ALICE)), Some(ALICE));
        assert_eq!(parse_hex("zz"), None);
    }
}
