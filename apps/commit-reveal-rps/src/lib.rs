use std::collections::HashMap;
use std::ops::Deref;
use std::str::FromStr;

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env};
use sha3::{Digest, Sha3_256};

#[derive(Debug, Serialize)]
pub enum Error {
    NotFound(String),
    ConversionError(String),
    WrongInput(String),
    InvalidCall(String),
}

#[app::event]
pub enum Event<'a> {
    PlayerUpdated { id: usize, hash: &'a [u8; 32] },
    PlayerInserted { id: usize, hash: &'a [u8; 32] },
}

#[app::state(emits = for<'a> Event<'a>)]
#[derive(Default, Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Game {
    players: HashMap<usize, Player>,
    ids: bool,
    commit_state: [bool; 2], //state for the two id's (did they commit)
    choices: [Option<String>; 2],
    results: [u8; 3],
}

#[app::state]
#[derive(Default, Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Player {
    hash: [u8; 32],
    reveal_correct: bool,
    id: usize,
}

#[derive(Debug)]
enum Choices {
    Rock,
    Paper,
    Scissors,
}

impl FromStr for Choices {
    type Err = Error;

    fn from_str(input: &str) -> Result<Choices, Self::Err> {
        match input {
            "Rock" => Ok(Choices::Rock),
            "Paper" => Ok(Choices::Paper),
            "Scissors" => Ok(Choices::Scissors),
            _ => Err(Error::ConversionError(
                "Choose one of Rock, Papar or Scissors".to_string(),
            )),
        }
    }
}

impl AsRef<[u8]> for Choices {
    fn as_ref(&self) -> &[u8] {
        match self {
            Choices::Rock => b"Rock",
            Choices::Paper => b"Paper",
            Choices::Scissors => b"Scissors",
        }
    }
}

#[app::logic]
impl Game {
    pub fn create_player_and_commit(
        &mut self,
        choice: &str,
        salt: &str,
        id: Option<usize>,
    ) -> Result<(), Error> {
        if self.players.len() == 2 && id == None {
            return Err(Error::WrongInput(
                "You have to provide your correct id".to_string(),
            ));
        }

        if id != None && self.commit_state[id.unwrap()] == true {
            return Err(Error::InvalidCall("You already commited".to_string()));
        }

        let hash = Game::calculate_hash(&choice, &salt)?;

        env::log(&format!("New hash is: {:?}", hash));

        //Generate an id
        let id_insert: usize;
        match id {
            Some(id) => {
                app::emit!(Event::PlayerUpdated {
                    id: id,
                    hash: &hash
                });
                let player = self
                    .players
                    .get_mut(&id)
                    .ok_or(Error::NotFound("Player with id not found".to_string()))?;
                player.hash = hash;
                player.reveal_correct = false;

                self.commit_state[id] = true;
                return Ok(());
            }
            None => match self.ids {
                true => id_insert = 1,
                false => {
                    id_insert = 0;
                    self.ids = true;
                }
            },
        }

        env::log(&format!("Your id is: {}", id_insert));

        app::emit!(Event::PlayerInserted {
            id: id_insert,
            hash: &hash
        });

        self.players.insert(
            id_insert,
            Player {
                hash,
                reveal_correct: false,
                id: id_insert,
            },
        );

        self.commit_state[id_insert] = true;

        env::log(&format!("{:?}", hash));
        Ok(())
    }

    pub fn reveal(&mut self, salt: &str, id: usize) -> Result<(), Error> {
        if self.commit_state.into_iter().any(|x| x == false) {
            return Err(Error::InvalidCall(
                "Can not reveal yet, one of the players did not commit".to_string(),
            ));
        }

        let player = self
            .players
            .get_mut(&id)
            .ok_or(Error::NotFound("Player with id not found".to_string()))?;

        player.reveal_correct = Game::compare_hashes(player.hash, salt).map(|x| {
            self.choices[id] = Some(x);
            return true;
        })?;

        if self.players.values().all(|t| t.reveal_correct == true) {
            let choice_0 = Choices::from_str(self.choices[0].as_ref().unwrap().as_str())?;
            let choice_1 = Choices::from_str(self.choices[1].as_ref().unwrap().as_str())?;

            match Game::determine_winner(choice_0, choice_1) {
                Some(id) => {
                    self.results[id] += 1;

                    self.commit_state = Default::default();
                    self.choices = Default::default();
                    env::log(&format!("Player {} won", id));
                    Ok(())
                }
                None => {
                    self.results[2] += 1;

                    self.commit_state = Default::default();
                    self.choices = Default::default();
                    env::log(&format!("The game ended in a draw"));
                    Ok(())
                }
            }
        } else {
            return Err(Error::NotFound(
                "One of the players salt wasn't correct or didn't reveal yet".to_string(),
            ));
        }
    }

    fn calculate_hash(choice: &str, salt: &str) -> Result<[u8; 32], Error> {
        Sha3_256::new()
            .chain_update(Choices::from_str(&choice)?)
            .chain_update(salt)
            .finalize()
            .deref()
            .try_into()
            .map_err(|_| Error::ConversionError("Failed to convert ".to_string()))
    }

    fn compare_hashes(hash: [u8; 32], salt: &str) -> Result<String, Error> {
        let choices = vec!["Rock", "Paper", "Scissors"];

        for choice in choices {
            if Game::calculate_hash(choice, &salt).unwrap() == hash {
                return Ok(choice.to_string());
            }
        }

        Err(Error::ConversionError(
            "No choices match the hash and salt".to_string(),
        ))
    }

    fn determine_winner(choice0: Choices, choice1: Choices) -> Option<usize> {
        match choice0 {
            Choices::Rock => match choice1 {
                Choices::Rock => None,
                Choices::Paper => Some(1),
                Choices::Scissors => Some(0),
            },
            Choices::Paper => match choice1 {
                Choices::Rock => Some(0),
                Choices::Paper => None,
                Choices::Scissors => Some(1),
            },
            Choices::Scissors => match choice1 {
                Choices::Rock => Some(1),
                Choices::Paper => Some(0),
                Choices::Scissors => None,
            },
        }
    }
}
