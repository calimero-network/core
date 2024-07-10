use std::io;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::DataType;
use crate::key;
use crate::slice::Slice;
use crate::types::PredefinedEntry;

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ApplicationMeta {
    // todo! impl proper entry reference count
    pub refs: usize,
    pub size: usize,
    pub source: Source,
}

// registry://near/testnet/miraclx.near/6aaf79da2b1e0a2a5573d48dc11fef1ae82c017d3678da105bed69cc60990142/0.1.0
// file:///home/joe/apps/application.wasm
// https://blobs.calimero.network/miraclx/myapp.wasm
#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct Source {
    pub scheme: Scheme,
    pub resource: Box<str>,
}

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum Scheme {
    Registry = 0,
    Other(Box<str>) = 1,
}

impl DataType<'_> for ApplicationMeta {
    type Error = io::Error;

    fn from_slice(slice: Slice) -> Result<Self, Self::Error> {
        borsh::from_slice(&slice)
    }

    fn as_slice(&self) -> Result<Slice, Self::Error> {
        borsh::to_vec(self).map(Into::into)
    }
}

impl PredefinedEntry for key::ApplicationMeta {
    type DataType<'a> = ApplicationMeta;
}
