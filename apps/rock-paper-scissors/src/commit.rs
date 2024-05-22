use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use sha3::{Digest, Sha3_256};

use crate::Choice;

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Commitment([u8; 32]);

pub type Nonce = [u8; 32];

impl Commitment {
    pub fn of(choice: Choice, nonce: &Nonce) -> Self {
        let mut hasher = Sha3_256::new();

        hasher.update(&[choice as u8]);
        hasher.update(nonce);

        Commitment(hasher.finalize().into())
    }

    pub const fn from_bytes(bytes: &[u8; 32]) -> Self {
        Commitment(*bytes)
    }

    pub const fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
}

impl AsRef<[u8]> for Commitment {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
