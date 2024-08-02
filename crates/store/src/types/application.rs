use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::Borsh;
use crate::key;
use crate::types::PredefinedEntry;

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ApplicationMeta {
    // todo! impl proper entry reference count
    // pub refs: usize,
    pub blob: key::BlobMeta,
    pub source: Option<Source>,
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
    File = 0,
    Http { secure: bool } = 1,
    Registry = 2,
    Other(Box<str>) = 3,
}

impl PredefinedEntry for key::ApplicationMeta {
    type Codec = Borsh;
    type DataType<'a> = ApplicationMeta;
}
