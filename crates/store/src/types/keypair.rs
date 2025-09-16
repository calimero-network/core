#![expect(single_use_lifetimes, reason = "borsh shenanigans")]

use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::Borsh;
use crate::key;
use crate::types::PredefinedEntry;

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct Keypair {
    pub public_key: [u8; 32],
    pub private_key: [u8; 32],
    pub alias: Option<Box<str>>,
}

impl Keypair {
    #[must_use]
    pub const fn new(public_key: [u8; 32], private_key: [u8; 32], alias: Option<Box<str>>) -> Self {
        Self {
            public_key,
            private_key,
            alias,
        }
    }
}

impl PredefinedEntry for key::Keypair {
    type Codec = Borsh;
    type DataType<'a> = Keypair;
}
