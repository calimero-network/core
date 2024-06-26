use std::io;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::DataType;
use crate::key;
use crate::slice::Slice;
use crate::types::PredefinedEntry;

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ContextMeta {}

impl DataType for ContextMeta {
    type Error = io::Error;

    fn from_slice(slice: Slice) -> Result<Self, Self::Error> {
        borsh::from_slice(&slice)
    }

    fn as_slice(&self) -> Result<Slice, Self::Error> {
        borsh::to_vec(self).map(Into::into)
    }
}

impl PredefinedEntry for key::ContextMeta {
    type DataType = ContextMeta;
}

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ContextState {}

impl DataType for ContextState {
    type Error = io::Error;

    fn from_slice(slice: Slice) -> Result<Self, Self::Error> {
        borsh::from_slice(&slice)
    }

    fn as_slice(&self) -> Result<Slice, Self::Error> {
        borsh::to_vec(self).map(Into::into)
    }
}

impl PredefinedEntry for key::ContextState {
    type DataType = ContextState;
}

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ContextIdentity {}

impl DataType for ContextIdentity {
    type Error = io::Error;

    fn from_slice(slice: Slice) -> Result<Self, Self::Error> {
        borsh::from_slice(&slice)
    }

    fn as_slice(&self) -> Result<Slice, Self::Error> {
        borsh::to_vec(self).map(Into::into)
    }
}

impl PredefinedEntry for key::ContextIdentity {
    type DataType = ContextIdentity;
}

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ContextTransaction {}

impl DataType for ContextTransaction {
    type Error = io::Error;

    fn from_slice(slice: Slice) -> Result<Self, Self::Error> {
        borsh::from_slice(&slice)
    }

    fn as_slice(&self) -> Result<Slice, Self::Error> {
        borsh::to_vec(self).map(Into::into)
    }
}

impl PredefinedEntry for key::ContextTransaction {
    type DataType = ContextTransaction;
}
