use std::io;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::{Borsh, DataType, View};
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

impl PredefinedEntry for key::ContextMeta {
    type DataType<'a> = View<ContextMeta, Borsh>;
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
    pub private_key: Option<[u8; 32]>,
}

impl PredefinedEntry for key::ContextIdentity {
    type DataType<'a> = View<ContextIdentity, Borsh>;
}

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ContextTransaction {
    pub method: Box<str>,
    pub payload: Box<[u8]>,
    pub prior_hash: TransactionHash,
}

impl PredefinedEntry for key::ContextTransaction {
    type DataType<'a> = View<ContextTransaction, Borsh>;
}
