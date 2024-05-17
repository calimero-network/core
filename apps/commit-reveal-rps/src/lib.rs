use std::cmp::Ordering;
use std::ops::Deref;

use calimero_sdk::app;
use calimero_sdk::borsh::{io, BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey, PUBLIC_KEY_LENGTH};
use sha3::{Digest, Sha3_256};

pub(crate) type Commitment = [u8; 32];

#[derive(Default, Debug)]
pub struct PublicKey(VerifyingKey);

impl BorshSerialize for PublicKey {
    fn serialize<W: std::io::Write>(&self, writer: &mut W) -> Result<(), std::io::Error> {
        let bytes = self.0.to_bytes();
        writer.write_all(&bytes)?;
        Ok(())
    }
}

impl BorshDeserialize for PublicKey {
    fn deserialize_reader<R: std::io::prelude::Read>(reader: &mut R) -> std::io::Result<Self> {
        let mut public_key_bytes = [0u8; PUBLIC_KEY_LENGTH];
        reader.read_exact(&mut public_key_bytes)?;

        let key = VerifyingKey::from_bytes(&public_key_bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid public key"))?;

        Ok(PublicKey(key))
    }
}

impl Serialize for PublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let bytes = self.0.to_bytes();
        serializer.serialize_bytes(&bytes)
    }
}

#[derive(Debug, Serialize)]
pub enum Error {
    ConversionError(String),
    VerifyError(String),
}

#[derive(Debug, Serialize)]
pub enum SignError {
    String,
}

#[derive(Debug, Serialize)]
pub enum JoinError {
    GameFull(String),
}

#[derive(Debug, Serialize)]
pub enum CommitError {
    PlayerNotFound(String),
    InvalidSignature(String),
    AlreadyCommitted(String),
}

#[derive(Debug, Serialize)]
pub enum RevealError {
    PlayerNotFound(String),
    InvalidNonce(String),
    NotCommitted(String),
    NotRevealed(String),
}

#[app::event]
pub enum Event<'a> {
    PlayerCommited {
        id: usize,
        hash: &'a [u8; 32],
    },
    PlayerInserted {
        id: usize,
        public_key: &'a PublicKey,
        name: &'a str,
    },
    PlayerRevealed {
        id: usize,
        reveal: &'a Choice,
    },
    PlayerWon {
        id: usize,
    },
    StateDumped {},
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
    key: PublicKey,
    name: String,
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
enum State {
    Commited(Commitment),
    Revealed(Choice),
}

#[derive(Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Deserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub enum Choice {
    Rock,
    Paper,
    Scissors,
}

impl AsRef<[u8]> for Choice {
    fn as_ref(&self) -> &[u8] {
        match self {
            Choice::Rock => b"Rock",
            Choice::Paper => b"Paper",
            Choice::Scissors => b"Scissors",
        }
    }
}

impl PartialOrd for Choice {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Choice {
    fn cmp(&self, other: &Self) -> Ordering {
        use Choice::*;

        match (self, other) {
            (Rock, Scissors) => Ordering::Greater,
            (Scissors, Paper) => Ordering::Greater,
            (Paper, Rock) => Ordering::Greater,

            (Scissors, Rock) => Ordering::Less,
            (Paper, Scissors) => Ordering::Less,
            (Rock, Paper) => Ordering::Less,

            _ => Ordering::Equal,
        }
    }
}

//#[app::logic]
impl Game {
    fn calculate_hash(choice: &Choice, salt: &str) -> Result<Commitment, Error> {
        Sha3_256::new()
            .chain_update(choice)
            .chain_update(salt)
            .finalize()
            .deref()
            .try_into()
            .map_err(|_| Error::ConversionError("Failed to convert ".to_string()))
    }

    fn compare_hashes(hash: Commitment, salt: &str) -> Result<Choice, Error> {
        let choices = vec![Choice::Rock, Choice::Paper, Choice::Scissors];

        for choice in choices {
            if Game::calculate_hash(&choice, &salt)? == hash {
                return Ok(choice);
            }
        }

        Err(Error::ConversionError(
            "No Choice match the hash and salt".to_string(),
        ))
    }

    fn create_keypair(random_bytes: &[u8; 32]) -> SigningKey {
        SigningKey::from_bytes(random_bytes)
    }

    fn sign(mut secret_key: SigningKey, message: &[u8]) -> Result<Signature, SignError> {
        Ok(secret_key.sign(message))
    }

    fn verify(
        &self,
        player_idx: usize,
        message: &[u8],
        signature: Signature,
    ) -> Result<bool, Error> {
        let signing_key = &self.players[player_idx]
            .as_ref()
            .ok_or(Error::VerifyError("Player not found".to_string()))?
            .key;
        Ok(signing_key.0.verify(message, &signature).is_ok())
    }

    pub fn join(&mut self, player_name: String, public_key: PublicKey) -> Result<usize, JoinError> {
        if let Some((index, slot)) = self
            .players
            .iter_mut()
            .enumerate()
            .find(|(_, player)| player.is_none())
        //find an empty spot in the game
        {
            app::emit!(Event::PlayerInserted {
                id: index,
                public_key: &public_key,
                name: &player_name
            });

            let new_player = Player {
                state: None,
                key: public_key,
                name: player_name,
            };

            *slot = Some(new_player);

            return Ok(index);
        } else {
            return Err(JoinError::GameFull(
                "Two players are already in the game".to_string(),
            ));
        }
    }

    fn state(&self) -> [Option<(String, &State)>; 2] {
        let mut states = [None, None];

        for (i, player) in self.players.as_ref().into_iter().enumerate() {
            match player {
                Some(p) => {
                    if let Some(player_state) = &p.state {
                        states[i] = Some((p.name.clone(), player_state));
                    }
                }
                None => continue,
            }
        }
        states
    }

    fn prepare(
        signing_key: &mut SigningKey,
        choice: Choice,
        salt: &str,
    ) -> Result<(Commitment, Signature), Error> {
        let hash: Commitment = Game::calculate_hash(&choice, salt)?;
        let signature = SigningKey::sign(signing_key, &hash);
        Ok((hash, signature))
    }

    fn commit(
        &mut self,
        player_idx: usize,
        commitment: Commitment,
        signature: Signature,
    ) -> Result<(), CommitError> {
        let player: &mut Player =
            self.players[player_idx]
                .as_mut()
                .ok_or(CommitError::PlayerNotFound(format!(
                    "Can't find player with idx {}",
                    player_idx
                )))?;

        if let Some(_) = player.state {
            return Err(CommitError::AlreadyCommitted(
                "Player already committed".to_string(),
            ));
        }

        match player.key.0.verify(&commitment, &signature) {
            Ok(_) => {
                app::emit!(Event::PlayerCommited {
                    id: player_idx,
                    hash: &commitment
                });
                player.state = Some(State::Commited(commitment));
                return Ok(());
            }
            Err(_) => Err(CommitError::InvalidSignature(
                "Couldn't verify the signature on a commitment".to_string(),
            )),
        }
    }

    fn reveal(&mut self, player_idx: usize, nonce: &str) -> Result<(), RevealError> {
        let player: &mut Player = self
            .players
            .get_mut(player_idx)
            .ok_or_else(|| {
                RevealError::PlayerNotFound(format!("Player with id {} not found", player_idx))
            })?
            .as_mut()
            .unwrap(); //this is infallible (?)

        if let Some(State::Commited(commitment)) = player.state {
            let choice = Game::compare_hashes(commitment, nonce).map_err(|_| {
                RevealError::InvalidNonce("The nonce provided is invalid".to_string())
            })?;
            app::emit!(Event::PlayerRevealed {
                id: player_idx,
                reveal: &choice
            });
            player.state = Some(State::Revealed(choice));
        } else {
            return Err(RevealError::NotCommitted(
                "Player has not committed yet".to_string(),
            ));
        }

        let revealed_choices = self
            .players
            .iter()
            .filter_map(|p| match p {
                Some(Player {
                    state: Some(State::Revealed(choice)),
                    ..
                }) => Some(choice),
                _ => None,
            })
            .collect::<Vec<&Choice>>();

        if revealed_choices.len() == 2 {
            let choice0 = *revealed_choices.get(0).ok_or(RevealError::NotRevealed(
                "Player with index 0 hasn't revealed yet".to_string(),
            ))?;
            let choice1 = *revealed_choices.get(1).ok_or(RevealError::NotRevealed(
                "Player with index 1 hasn't revealed yet".to_string(),
            ))?;

            if choice0 == choice1 {
                app::emit!(Event::PlayerWon { id: 3 });
                return Ok(());
            }
            if choice0 > choice1 {
                app::emit!(Event::PlayerWon { id: 0 });
                return Ok(());
            } else {
                app::emit!(Event::PlayerWon { id: 1 });
                return Ok(());
            }
        } else {
            return Err(RevealError::NotRevealed(
                "One of the players hasn't revealed yet".to_string(),
            ));
        }
    }

    fn reset(&mut self) {
        self.players[0] = None;
        self.players[1] = None;
        app::emit!(Event::StateDumped {})
    }
}
