use std::cmp::Ordering;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;

mod choice;
mod commit;
mod errors;
mod key;
mod player_idx;
mod repr;

use choice::Choice;
use commit::{Commitment, Nonce};
use errors::{CommitError, JoinError, ResetError, RevealError};
use key::KeyComponents;
use player_idx::PlayerIdx;
use repr::Repr;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Default, Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Game {
    players: [Option<Player>; 2],
}

#[derive(Default, Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Player {
    state: Option<State>,
    public_key: Repr<VerifyingKey, repr::Raw>,
    name: String,
}

#[derive(Debug, Clone, PartialEq, BorshSerialize, BorshDeserialize, Deserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
enum State {
    Committed(Repr<Commitment>),
    Revealed(Choice),
}

#[app::event]
pub enum Event<'a> {
    PlayerCommited { id: PlayerIdx },
    NewPlayer { id: PlayerIdx, name: &'a str },
    PlayerRevealed { id: PlayerIdx, reveal: &'a Choice },
    GameOver { winner: Option<PlayerIdx> },
    StateDumped,
}

pub type Seed = [u8; 32];

#[app::logic]
impl Game {
    pub fn create_keypair(seed: Seed) -> KeyComponents {
        let mut csprng = ChaCha20Rng::from_seed(seed);

        let keypair = SigningKey::generate(&mut csprng);

        KeyComponents {
            pk: Repr::from(keypair.verifying_key()),
            sk: Repr::from(keypair),
        }
    }

    pub fn join(
        &mut self,
        player_name: String,
        public_key: Repr<VerifyingKey>,
    ) -> Result<usize, JoinError> {
        let Some((index, player)) = self
            .players
            .iter_mut()
            .enumerate()
            .find(|(_, player)| player.is_none())
        else {
            return Err(JoinError::GameFull);
        };

        app::emit!(Event::NewPlayer {
            id: PlayerIdx(index),
            name: &player_name
        });

        *player = Some(Player {
            state: None,
            public_key: Repr::from(public_key),
            name: player_name,
        });

        Ok(index)
    }

    pub fn state(&self) -> [Option<(&str, &State)>; 2] {
        let mut states = [None, None];

        for (i, player) in self.players.iter().enumerate() {
            if let Some(Player {
                state: Some(state),
                name,
                ..
            }) = player
            {
                states[i] = Some((name.as_str(), state));
            }
        }

        states
    }

    pub fn prepare(
        signing_key: Repr<SigningKey>,
        choice: Choice,
        nonce: Nonce,
    ) -> (Repr<Commitment>, Repr<Signature>) {
        let commitment = Commitment::of(choice, &nonce);

        let signature = signing_key.sign(commitment.as_ref());

        (Repr::from(commitment), Repr::from(signature))
    }

    fn players(&mut self, my_idx: PlayerIdx) -> (Option<&mut Player>, Option<&mut Player>) {
        let [a, b] = self.players.each_mut();
        if my_idx.is_first() {
            return (a.as_mut(), b.as_mut());
        }
        (b.as_mut(), a.as_mut())
    }

    pub fn commit(
        &mut self,
        player_idx: PlayerIdx,
        commitment: Repr<Commitment>,
        signature: Repr<Signature>,
    ) -> Result<(), CommitError> {
        let (Some(player), Some(_)) = self.players(player_idx) else {
            return Err(CommitError::NotReady);
        };

        if player.state.is_some() {
            return Err(CommitError::AlreadyCommitted);
        }

        player
            .public_key
            .verify(commitment.as_ref(), &signature)
            .map_err(|_| CommitError::InvalidSignature)?;

        app::emit!(Event::PlayerCommited { id: player_idx });

        player.state = Some(State::Committed(commitment));

        Ok(())
    }

    pub fn reveal(&mut self, player_idx: PlayerIdx, nonce: Nonce) -> Result<(), RevealError> {
        let (Some(player), Some(other_player)) = self.players(player_idx) else {
            return Err(RevealError::NotReady);
        };

        let Some(State::Committed(commitment)) = &player.state else {
            return Err(RevealError::NotCommitted);
        };

        let choice = Choice::determine(commitment, &nonce).ok_or(RevealError::InvalidNonce)?;

        app::emit!(Event::PlayerRevealed {
            id: player_idx,
            reveal: &choice
        });

        player.state = Some(State::Revealed(choice));

        if let Some(State::Revealed(other)) = &other_player.state {
            match choice.partial_cmp(other) {
                Some(Ordering::Less) => app::emit!(Event::GameOver {
                    winner: Some(player_idx.other())
                }),
                Some(Ordering::Equal) => app::emit!(Event::GameOver { winner: None }),
                Some(Ordering::Greater) => app::emit!(Event::GameOver {
                    winner: Some(player_idx)
                }),
                None => {}
            }
        }

        Ok(())
    }

    pub fn reset(
        &mut self,
        player_idx: PlayerIdx,
        commitment: Repr<Commitment>,
        signature: Repr<Signature>,
    ) -> Result<(), ResetError> {
        let (Some(player), _) = self.players(player_idx) else {
            return Err(ResetError::NotReady);
        };

        player
            .public_key
            .verify(commitment.as_ref(), &signature)
            .map_err(|_| ResetError::InvalidSignature)?;

        self.players = Default::default();

        app::emit!(Event::StateDumped);

        Ok(())
    }
}
