use std::cmp::Ordering;

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};

use crate::commit::{Commitment, Nonce};

#[derive(
    Copy, Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize, Deserialize, Serialize,
)]
#[borsh(crate = "calimero_sdk::borsh")]
#[serde(crate = "calimero_sdk::serde")]
#[repr(u8)]
pub enum Choice {
    Rock,
    Paper,
    Scissors,
}

impl Choice {
    pub fn determine(commitment: &Commitment, nonce: &Nonce) -> Option<Self> {
        let choices = [Choice::Rock, Choice::Paper, Choice::Scissors];

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
