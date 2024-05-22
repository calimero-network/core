use std::cmp::Ordering;

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};

use crate::commit::{Commitment, Nonce};

#[derive(
    Eq, Copy, Clone, Debug, PartialEq, Serialize, Deserialize, BorshSerialize, BorshDeserialize,
)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
#[repr(u8)]
pub enum Choice {
    Rock,
    Paper,
    Scissors,
}

use Choice::*;

impl Choice {
    pub fn determine(commitment: &Commitment, nonce: &Nonce) -> Option<Self> {
        let choices = [Rock, Paper, Scissors];

        for choice in choices {
            if *commitment == Commitment::of(choice, nonce) {
                return Some(choice);
            }
        }

        None
    }
}

impl PartialOrd for Choice {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Rock, Scissors) | (Scissors, Paper) | (Paper, Rock) => Some(Ordering::Greater),
            (Scissors, Rock) | (Paper, Scissors) | (Rock, Paper) => Some(Ordering::Less),
            _ => Some(Ordering::Equal),
        }
    }
}
