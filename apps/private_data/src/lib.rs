#![allow(clippy::len_without_is_empty)]

use std::collections::BTreeMap;

use bs58;
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct SecretGame {
    /// Mapping of game_id -> sha256(secret) hex
    games: UnorderedMap<String, LwwRegister<String>>,
}

#[derive(BorshSerialize, BorshDeserialize, Debug)]
#[borsh(crate = "calimero_sdk::borsh")]
#[app::private]
pub struct Secrets {
    /// Private mapping of game_id -> secret
    /// Note: Private data is node-local and NOT synchronized, so we can use String directly
    secrets: UnorderedMap<String, String>,
}

impl Default for Secrets {
    fn default() -> Self {
        Self {
            secrets: UnorderedMap::new(),
        }
    }
}

#[app::event]
pub enum Event<'a> {
    SecretSet {
        game_id: &'a str,
    },
    Guessed {
        game_id: &'a str,
        success: bool,
        by: &'a str,
    },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("no public hash set yet")]
    NoHash,
    #[error("utf8 error: {0}")]
    Utf8(&'a str),
}

#[app::logic]
impl SecretGame {
    #[app::init]
    pub fn init() -> SecretGame {
        SecretGame {
            games: UnorderedMap::new(),
        }
    }

    /// Create/update a game by id: store secret privately and record its hash publicly.
    pub fn add_secret(&mut self, game_id: String, secret: String) -> app::Result<()> {
        // Save private secret using the Secrets private storage
        let mut secrets = Secrets::private_load_or_default()?;
        let mut secrets_mut = secrets.as_mut();
        secrets_mut
            .secrets
            .insert(game_id.clone(), secret.clone())?;

        // Save public hash for guess verification in games map
        let hash = Sha256::digest(secret.as_bytes());
        let hash_hex = hex::encode(hash);
        self.games.insert(game_id.clone(), hash_hex.into())?;
        app::emit!(Event::SecretSet { game_id: &game_id });
        Ok(())
    }

    /// Allow a user to guess the secret; returns true if the guess matches the stored hash.
    /// `who` is derived from the executor identity rather than passed as an argument.
    pub fn add_guess(&self, game_id: &str, guess: String) -> app::Result<bool> {
        let Some(public_hash_hex) = self.games.get(game_id)?.map(|v| v.get().clone()) else {
            app::bail!(Error::NoHash);
        };
        let guess_hash = Sha256::digest(guess.as_bytes());
        let guess_hash_hex = hex::encode(guess_hash);
        let who_b = calimero_sdk::env::executor_id();
        let who = bs58::encode(who_b).into_string();
        let success = guess_hash_hex == public_hash_hex;
        app::emit!(Event::Guessed {
            game_id,
            success,
            by: &who
        });
        Ok(success)
    }

    /// Get all local secrets from private storage for the current caller.
    pub fn my_secrets(&self) -> app::Result<BTreeMap<String, String>> {
        let secrets = Secrets::private_load_or_default()?;
        let map: BTreeMap<_, _> = secrets.secrets.entries()?.collect();
        Ok(map)
    }

    /// Get all public games and their secret hashes.
    pub fn games(&self) -> app::Result<BTreeMap<String, String>> {
        Ok(self
            .games
            .entries()?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }
}
