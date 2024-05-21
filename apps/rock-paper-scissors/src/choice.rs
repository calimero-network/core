use std::cmp::Ordering;

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};

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
