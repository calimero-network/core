use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

pub mod constants;
pub use constants::{root_storage_key, DIGEST_SIZE, ROOT_STORAGE_ENTRY_ID};

#[derive(
    Debug, Clone, Copy, BorshSerialize, BorshDeserialize, Serialize, Deserialize, PartialEq, Eq,
)]
#[borsh(crate = "borsh")]
#[serde(crate = "serde")]
pub struct PublicKey(pub [u8; 32]);

impl From<[u8; 32]> for PublicKey {
    fn from(array: [u8; 32]) -> Self {
        Self(array)
    }
}

impl AsRef<[u8]> for PublicKey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
