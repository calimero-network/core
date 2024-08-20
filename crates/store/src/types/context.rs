use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::{KeyPair, PublicKey};

use crate::entry::{Borsh, Identity};
use crate::key;
use crate::slice::Slice;
use crate::types::PredefinedEntry;

pub type TransactionHash = [u8; 32];

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ContextMeta {
    pub application: key::ApplicationMeta,
    pub last_transaction_hash: TransactionHash,
}

impl PredefinedEntry for key::ContextMeta {
    type Codec = Borsh;
    type DataType<'a> = ContextMeta;
}

#[derive(Eq, Clone, Debug, PartialEq)]
pub struct ContextState<'a> {
    pub value: Slice<'a>,
}

impl PredefinedEntry for key::ContextState {
    type Codec = Identity;
    type DataType<'a> = ContextState<'a>;
}

impl<'a> From<Slice<'a>> for ContextState<'a> {
    fn from(value: Slice<'a>) -> Self {
        Self { value }
    }
}

impl AsRef<[u8]> for ContextState<'_> {
    fn as_ref(&self) -> &[u8] {
        self.value.as_ref()
    }
}

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ContextIdentity {
    pub public_key: PublicKey,
    pub private_key: Option<[u8; 32]>,
}

impl From<KeyPair> for ContextIdentity {
    fn from(id: KeyPair) -> Self {
        Self {
            public_key: id.public_key,
            private_key: id.private_key,
        }
    }
}

impl From<ContextIdentity> for KeyPair {
    fn from(id: ContextIdentity) -> Self {
        Self {
            public_key: id.public_key,
            private_key: id.private_key,
        }
    }
}

impl PredefinedEntry for key::ContextIdentity {
    type Codec = Borsh;
    type DataType<'a> = ContextIdentity;
}

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ContextTransaction {
    pub method: Box<str>,
    pub payload: Box<[u8]>,
    pub prior_hash: TransactionHash,
    pub executor_public_key: [u8; 32],
}

impl PredefinedEntry for key::ContextTransaction {
    type Codec = Borsh;
    type DataType<'a> = ContextTransaction;
}
