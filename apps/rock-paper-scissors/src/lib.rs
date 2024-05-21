use std::cmp::Ordering;
use std::marker::PhantomData;

use calimero_sdk::app;
use calimero_sdk::borsh::{io, BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey, PUBLIC_KEY_LENGTH};
use rand_chacha::rand_core::SeedableRng;
use rand_chacha::ChaCha20Rng;
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
        S: calimero_sdk::serde::Serializer,
    {
        let bytes = self.0.to_bytes();
        let encoded = bs58::encode(bytes).into_string();
        serializer.serialize_str(&encoded)
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: calimero_sdk::serde::Deserializer<'de>,
    {
        let encoded = <std::string::String as Deserialize>::deserialize(deserializer)?;
        let bytes_decoded = bs58::decode(&encoded)
            .into_vec()
            .map_err(|_| calimero_sdk::serde::de::Error::custom("Error"))?;

        if bytes_decoded.len() != PUBLIC_KEY_LENGTH {
            return Err(calimero_sdk::serde::de::Error::custom(
                "Invalid public key length",
            ));
        }

        let mut public_key_bytes = [0u8; PUBLIC_KEY_LENGTH];
        public_key_bytes.copy_from_slice(&bytes_decoded);

        let key = VerifyingKey::from_bytes(&public_key_bytes)
            .map_err(|_| calimero_sdk::serde::de::Error::custom("Invalid public key"))?;

        Ok(PublicKey(key))
    }
}

pub trait AsKeyBytes {
    fn as_key_bytes(&self) -> &[u8];
}

impl AsKeyBytes for PublicKey {
    fn as_key_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl AsKeyBytes for SigningKey {
    fn as_key_bytes(&self) -> &[u8] {
        self.as_bytes()
    }
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "calimero_sdk::serde")]
pub struct KeyComponent<T: AsKeyBytes> {
    #[serde(
        serialize_with = "serialize_base58",
        deserialize_with = "deserialize_base58"
    )]
    key_bytes: Vec<u8>,
    #[serde(skip)]
    _marker: PhantomData<T>,
}

impl<T: AsKeyBytes> From<T> for KeyComponent<T> {
    fn from(key: T) -> Self {
        KeyComponent {
            key_bytes: key.as_key_bytes().to_vec(),
            _marker: PhantomData,
        }
    }
}

fn serialize_base58<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: calimero_sdk::serde::Serializer,
{
    let encoded = bs58::encode(bytes).into_string();
    serializer.serialize_str(&encoded)
}

fn deserialize_base58<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: calimero_sdk::serde::Deserializer<'de>,
{
    let s = <std::string::String as Deserialize>::deserialize(deserializer)?;
    let decoded = bs58::decode(&s)
        .into_vec()
        .map_err(calimero_sdk::serde::de::Error::custom)?;
    Ok(decoded)
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "calimero_sdk::serde")]

pub struct KeyComponents {
    pub pk: KeyComponent<PublicKey>,
    pub sk: KeyComponent<SigningKey>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Serialize, Deserialize)]
#[serde(crate = "calimero_sdk::serde")]
struct PlayerIdx(usize);

impl PlayerIdx {
    fn new(value: usize) -> Result<Self, &'static str> {
        match value {
            0 | 1 => Ok(PlayerIdx(value)),
            _ => Err("Player index must be either 0 or 1."),
        }
    }

    fn value(self) -> usize {
        self.0
    }
}

impl Into<usize> for PlayerIdx {
    fn into(self) -> usize {
        self.0
    }
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub enum Error {
    ConversionError,
    ResetError,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub enum JoinError {
    GameFull,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub enum CommitError {
    OtherNotJoined,
    PlayerNotFound,
    InvalidSignature,
    AlreadyCommitted,
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub enum RevealError {
    PlayerNotFound,
    InvalidNonce,
    NotCommitted,
    NotRevealed,
}

#[app::event]
pub enum Event<'a> {
    PlayerCommited { id: usize },
    NewPlayer { id: usize, name: &'a str },
    PlayerRevealed { id: usize, reveal: &'a Choice },
    PlayerWon { id: usize },
    StateDumped,
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

#[derive(Debug, Clone, PartialEq, BorshSerialize, BorshDeserialize, Deserialize, Serialize)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
enum State {
    Commited(Commitment),
    Revealed(Choice),
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Deserialize, Serialize,
)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
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
        use Choice::*;
        match (self, other) {
            (Rock, Scissors) => Some(Ordering::Greater),
            (Scissors, Paper) => Some(Ordering::Greater),
            (Paper, Rock) => Some(Ordering::Greater),

            (Scissors, Rock) => Some(Ordering::Less),
            (Paper, Scissors) => Some(Ordering::Less),
            (Rock, Paper) => Some(Ordering::Less),

            _ => Some(Ordering::Equal),
        }
    }
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

    fn compare_hashes(hash: Commitment, salt: &str) -> Result<Choice, Error> {
        let choices: [Choice; 3] = [Choice::Rock, Choice::Paper, Choice::Scissors];

        for choice in choices {
            if Game::calculate_hash(&choice, &salt) == hash {
                return Ok(choice);
            }
        }

        Err(Error::ConversionError)
    }

    pub fn create_keypair(random_bytes: [u8; 32]) -> KeyComponents {
        let mut csprng = ChaCha20Rng::from_seed(random_bytes);
        let keypair = SigningKey::generate(&mut csprng);
        KeyComponents {
            pk: KeyComponent::from(PublicKey(keypair.verifying_key())),
            sk: KeyComponent::from(keypair),
        }
    }

    pub fn sign(mut secret_key: SigningKey, message: &[u8]) -> Signature {
        secret_key.sign(message)
    }

    pub fn verify(
        &self,
        player_idx: PlayerIdx,
        message: &[u8],
        signature: Signature,
    ) -> Option<bool> {
        let signing_key = &self.players[player_idx.value()]
            .as_ref()
            .or_else(|| return None)
            .unwrap()
            .key;

        Some(signing_key.0.verify(message, &signature).is_ok())
    }

    pub fn join(&mut self, player_name: String, public_key: PublicKey) -> Result<usize, JoinError> {
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
            key: public_key,
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
        signing_key: SigningKey,
        choice: Choice,
        nonce: &str,
    ) -> Result<(Commitment, Signature), Error> {
        let hash: Commitment = Game::calculate_hash(&choice, nonce);
        let signature = SigningKey::sign(&signing_key, &hash);
        Ok((hash, signature))
    }

    pub fn commit(
        &mut self,
        player_idx: PlayerIdx,
        commitment: Commitment,
        signature: Signature,
    ) -> Result<(), CommitError> {
        if self.players[(player_idx.value() + 1) % 2].is_none() {
            return Err(CommitError::OtherNotJoined);
        }

        let player: &mut Player = self.players[player_idx.value() as usize]
            .as_mut()
            .ok_or(CommitError::PlayerNotFound)?;

        if let Some(_) = player.state {
            return Err(CommitError::AlreadyCommitted);
        }

        match player.key.0.verify(&commitment, &signature) {
            Ok(_) => {
                app::emit!(Event::PlayerCommited {
                    id: player_idx.into(),
                });
                player.state = Some(State::Commited(commitment));
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
            .ok_or_else(|| RevealError::PlayerNotFound)?
            .as_mut()
            .unwrap();

        if let Some(State::Commited(commitment)) = player.state {
            choice =
                Game::compare_hashes(commitment, nonce).map_err(|_| RevealError::InvalidNonce)?;
            app::emit!(Event::PlayerRevealed {
                id: player_idx.into(),
                reveal: &choice
            });
            player.state = Some(State::Revealed(choice));
        } else {
            return Err(RevealError::NotCommitted);
        }

        let other_idx = (player_idx.value() + 1) % 2;
        if let Some(other_player) = &self.players[other_idx] {
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
            app::emit!(Event::PlayerWon { id: 3 });
        }
        if choice0 > choice1 {
            app::emit!(Event::PlayerWon { id: 0 });
        } else {
            app::emit!(Event::PlayerWon { id: 1 });
        }
    }
    pub fn reset(
        &mut self,
        player_idx: PlayerIdx,
        message: &[u8],
        signature: Signature,
    ) -> Result<(), Error> {
        if self.verify(player_idx, message, signature).is_none() {
            return Err(Error::ResetError);
        }

        self.players = Default::default();
        app::emit!(Event::StateDumped {});
        Ok(())
    }
}
