use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;
use sha3::{Digest, Sha3_256};

mod choice;
mod errors;
mod key_component;
mod keys;
mod player_idx;

use choice::Choice;
use errors::{CommitError, Error, JoinError, RevealError};
use key_component::KeyComponent;
use player_idx::PlayerIdx;

pub(crate) type Commitment = [u8; 32];

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "calimero_sdk::serde")]
pub struct KeyComponents {
    pub pk: KeyComponent<VerifyingKey>,
    pub sk: KeyComponent<SigningKey>,
}

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Default, Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Game {
    players: [Option<Player>; 2],
}

#[app::state]
#[derive(Default, Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Player {
    state: Option<State>,
    public_key: KeyComponent<VerifyingKey>,
    name: String,
}

#[derive(Debug, Clone, PartialEq, BorshSerialize, BorshDeserialize, Deserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
enum State {
    Commited(Commitment),
    Revealed(Choice),
}

#[app::event]
pub enum Event<'a> {
    PlayerCommited { id: usize },
    NewPlayer { id: usize, name: &'a str },
    PlayerRevealed { id: usize, reveal: &'a Choice },
    GameOver { winner: Option<usize> },
    StateDumped,
}

#[app::logic]
impl Game {
    fn calculate_hash(choice: &Choice, salt: &str) -> Commitment {
        Sha3_256::new()
            .chain_update(choice)
            .chain_update(salt)
            .finalize()
            .into()
    }

    fn compare_hashes(hash: Commitment, salt: &str) -> Option<Choice> {
        let choices: [Choice; 3] = [Choice::Rock, Choice::Paper, Choice::Scissors];

        for choice in choices {
            if Game::calculate_hash(&choice, &salt) == hash {
                return Some(choice);
            }
        }

        None
    }

    pub fn create_keypair(random_bytes: [u8; 32]) -> KeyComponents {
        let mut csprng = ChaCha20Rng::from_seed(random_bytes);
        let keypair = SigningKey::generate(&mut csprng);
        KeyComponents {
            pk: KeyComponent::from(keypair.verifying_key()),
            sk: KeyComponent::from(keypair),
        }
    }

    pub fn sign(secret_key: KeyComponent<SigningKey>, message: &[u8]) -> KeyComponent<Signature> {
        KeyComponent {
            key: secret_key.key.sign(message),
        }
    }

    pub fn verify(
        &self,
        player_idx: PlayerIdx,
        message: &[u8],
        signature: KeyComponent<Signature>,
    ) -> Option<bool> {
        let signing_key = &self.players[player_idx.value()].as_ref()?.public_key;

        Some(signing_key.key.verify(message, &signature.key).is_ok())
    }

    pub fn join(
        &mut self,
        player_name: String,
        public_key: KeyComponent<VerifyingKey>,
    ) -> Result<usize, JoinError> {
        let Some((index, slot)) = self
            .players
            .iter_mut()
            .enumerate()
            .find(|(_, player)| player.is_none())
        else {
            return Err(JoinError::GameFull);
        };

        app::emit!(Event::NewPlayer {
            id: index,
            name: &player_name
        });

        let new_player = Player {
            state: None,
            public_key: KeyComponent {
                key: public_key.key,
            },
            name: player_name,
        };

        *slot = Some(new_player);

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
        signing_key: KeyComponent<SigningKey>,
        choice: Choice,
        nonce: &str,
    ) -> Result<(Commitment, Signature), ()> {
        let hash: Commitment = Game::calculate_hash(&choice, nonce);
        let signature = SigningKey::sign(&signing_key.key, &hash);
        Ok((hash, signature))
    }

    pub fn commit(
        &mut self,
        player_idx: PlayerIdx,
        commitment: KeyComponent<Commitment>,
        signature: KeyComponent<Signature>,
    ) -> Result<(), CommitError> {
        let (player, other_player) = self
            .players_mut(player_idx)
            .ok_or(CommitError::OtherNotJoined)?;

        if let Some(_) = player.state {
            return Err(CommitError::AlreadyCommitted);
        }

        match player
            .public_key
            .key
            .verify(&commitment.key, &signature.key)
        {
            Ok(_) => {
                app::emit!(Event::PlayerCommited {
                    id: player_idx.value(),
                });
                player.state = Some(State::Commited(commitment.key));
                return Ok(());
            }
            Err(_) => Err(CommitError::InvalidSignature),
        }
    }

    pub fn reveal(&mut self, player_idx: PlayerIdx, nonce: &str) -> Result<(), RevealError> {
        let choice: Choice;

        let (player, other_player) = self.players(player_idx).ok_or(RevealError::NotCommitted)?;

        if let Some(State::Commited(commitment)) = player.state {
            choice = Game::compare_hashes(commitment, nonce).ok_or(RevealError::InvalidNonce)?;
            app::emit!(Event::PlayerRevealed {
                id: player_idx.value(),
                reveal: &choice
            });
            player.state = Some(State::Revealed(choice));
        } else {
            return Err(RevealError::NotCommitted);
        }

        if let Some(State::Revealed(other_choice)) = &other_player.state {
            Game::determine_winner(&choice, other_choice);
            return Ok(());
        } else {
            return Err(RevealError::NotRevealed);
        }
    }

    fn determine_winner(choice0: &Choice, choice1: &Choice) {
        match choice0.partial_cmp(choice1) {
            Some(result) => match result {
                std::cmp::Ordering::Less => app::emit!(Event::GameOver { winner: Some(1) }),
                std::cmp::Ordering::Equal => app::emit!(Event::GameOver { winner: None }),
                std::cmp::Ordering::Greater => app::emit!(Event::GameOver { winner: Some(0) }),
            },
            None => (),
        };
    }
    pub fn reset(
        &mut self,
        player_idx: PlayerIdx,
        message: &[u8],
        signature: KeyComponent<Signature>,
    ) -> Result<(), Error> {
        if self.verify(player_idx, message, signature).is_none() {
            return Err(Error::ResetError);
        }

        self.players = Default::default();
        app::emit!(Event::StateDumped);
        Ok(())
    }

    fn players(&self, my_idx: PlayerIdx) -> Option<(&Player, &Player)> {
        let my_idx = my_idx.value();
        let other_idx = (my_idx + 1) % 2;

        match (
            self.players[my_idx].as_ref(),
            self.players[other_idx].as_ref(),
        ) {
            (Some(player), Some(other_player)) => Some((player, other_player)),
            _ => None,
        }
    }

    fn players_mut(&mut self, my_idx: PlayerIdx) -> Option<(&mut Player, &mut Player)> {
        let my_idx = my_idx.value();
        let other_idx = (my_idx + 1) % 2;

        match (
            self.players[my_idx].as_mut(),
            self.players[other_idx].as_mut(),
        ) {
            (Some(player), Some(other_player)) => Some((player, other_player)),
            _ => None,
        }
    }
}
