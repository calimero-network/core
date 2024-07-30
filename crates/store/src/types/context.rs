use std::io;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::{KeyPair, PublicKey};

use crate::entry::DataType;
use crate::key;
use crate::slice::Slice;
use crate::types::PredefinedEntry;

pub type TransactionHash = [u8; 32];

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ContextMeta {
    // todo! make [u8; 32] when application_id<->meta is a separate record
    pub application_id: Box<str>,
    pub last_transaction_hash: TransactionHash,
}

impl DataType<'_> for ContextMeta {
    type Error = io::Error;

    fn from_slice(slice: Slice) -> Result<Self, Self::Error> {
        borsh::from_slice(&slice)
    }

    fn as_slice(&self) -> Result<Slice, Self::Error> {
        borsh::to_vec(self).map(Into::into)
    }
}

impl PredefinedEntry for key::ContextMeta {
    type DataType<'a> = ContextMeta;
}

#[derive(Eq, Clone, Debug, PartialEq)]
pub struct ContextState<'a> {
    pub value: Slice<'a>,
}

impl<'a> DataType<'a> for ContextState<'a> {
    type Error = io::Error;

    fn from_slice(slice: Slice<'a>) -> Result<Self, Self::Error> {
        Ok(Self { value: slice })
    }

    fn as_slice(&'a self) -> Result<Slice<'a>, Self::Error> {
        Ok(self.value.as_ref().into())
    }
}

impl PredefinedEntry for key::ContextState {
    type DataType<'a> = ContextState<'a>;
}

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ContextIdentity {
    pub public_key: PublicKey,
    pub private_key: Option<[u8; 32]>,
}

impl DataType<'_> for ContextIdentity {
    type Error = io::Error;

    fn from_slice(slice: Slice) -> Result<Self, Self::Error> {
        borsh::from_slice(&slice)
    }

    fn as_slice(&self) -> Result<Slice, Self::Error> {
        borsh::to_vec(self).map(Into::into)
    }
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
    type DataType<'a> = ContextIdentity;
}

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ContextTransaction {
    pub method: Box<str>,
    pub payload: Box<[u8]>,
    pub prior_hash: TransactionHash,
    pub executor_public_key: [u8; 32],
}

impl DataType<'_> for ContextTransaction {
    type Error = io::Error;

    fn from_slice(slice: Slice) -> Result<Self, Self::Error> {
        borsh::from_slice(&slice)
    }

    fn as_slice(&self) -> Result<Slice, Self::Error> {
        borsh::to_vec(self).map(Into::into)
    }
}

impl PredefinedEntry for key::ContextTransaction {
    type DataType<'a> = ContextTransaction;
}
