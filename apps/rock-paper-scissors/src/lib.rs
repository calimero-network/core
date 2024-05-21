mod choice;
mod errors;
mod events;
mod key_component;
mod keys;
mod player_idx;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use choice::Choice;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use errors::{CommitError, Error, JoinError, RevealError};
use events::Event;
use key_component::KeyComponent;
use player_idx::PlayerIdx;
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;
use sha3::{Digest, Sha3_256};

pub(crate) type Commitment = [u8; 32];

pub(crate) type PublicKey = VerifyingKey;

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "calimero_sdk::serde")]
pub struct KeyComponents {
    pub pk: KeyComponent<PublicKey>,
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
    public_key: KeyComponent<PublicKey>,
    name: String,
}

#[derive(Debug, Clone, PartialEq, BorshSerialize, BorshDeserialize, Deserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
enum State {
    Commited(Commitment),
    Revealed(Choice),
}

#[app::logic]
impl Game {
    fn calculate_hash(choice: &Choice, salt: &str) -> KeyComponent<Commitment> {
        KeyComponent {
            key: Sha3_256::new()
                .chain_update(choice)
                .chain_update(salt)
                .finalize()
                .into(),
        }
    }

    fn compare_hashes(hash: KeyComponent<Commitment>, salt: &str) -> Option<Choice> {
        let choices: [Choice; 3] = [Choice::Rock, Choice::Paper, Choice::Scissors];

        for choice in choices {
            if Game::calculate_hash(&choice, &salt) == hash {
                return Some(choice);
            }
        }

        None
    }

    pub fn create_keypair(random_bytes: KeyComponent<[u8; 32]>) -> KeyComponents {
        let mut csprng = ChaCha20Rng::from_seed(random_bytes.key);
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
        public_key: KeyComponent<PublicKey>,
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
    ) -> Result<(KeyComponent<Commitment>, KeyComponent<Signature>), Error> {
        let hash: Commitment = Game::calculate_hash(&choice, nonce).key;
        let signature = SigningKey::sign(&signing_key.key, &hash);
        Ok((KeyComponent { key: hash }, KeyComponent { key: signature }))
    }

    pub fn commit(
        &mut self,
        player_idx: PlayerIdx,
        commitment: KeyComponent<Commitment>,
        signature: KeyComponent<Signature>,
    ) -> Result<(), CommitError> {
        if self.other_player(player_idx).is_none() {
            return Err(CommitError::OtherNotJoined);
        }

        let player: &mut Player = self.players[player_idx.value() as usize]
            .as_mut()
            .ok_or(CommitError::PlayerNotFound)?;

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

        let player: &mut Player = self
            .players
            .get_mut(player_idx.value())
            .ok_or(RevealError::PlayerNotFound)?
            .as_mut()
            .ok_or(RevealError::PlayerNotFound)?;

        if let Some(State::Commited(commitment)) = player.state {
            choice = Game::compare_hashes(KeyComponent { key: commitment }, nonce)
                .ok_or(RevealError::InvalidNonce)?;
            app::emit!(Event::PlayerRevealed {
                id: player_idx.value(),
                reveal: &choice
            });
            player.state = Some(State::Revealed(choice));
        } else {
            return Err(RevealError::NotCommitted);
        }

        if let Some(other_player) = self.other_player(player_idx) {
            if let Some(State::Revealed(other_choice)) = &other_player.state {
                Game::determine_winner(&choice, other_choice);
                return Ok(());
            } else {
                return Err(RevealError::NotRevealed);
            }
        } else {
            return Err(RevealError::PlayerNotFound);
        }
    }

    fn determine_winner(choice0: &Choice, choice1: &Choice) {
        if choice0 == choice1 {
            app::emit!(Event::GameOver(None));
        }
        if choice0 > choice1 {
            app::emit!(Event::GameOver(Some(0)));
        } else {
            app::emit!(Event::GameOver(Some(1)));
        }
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

    fn other_player(&self, my_idx: PlayerIdx) -> Option<&Player> {
        let other_idx = (my_idx.value() + 1) % 2;
        self.players[other_idx].as_ref()
    }
}
