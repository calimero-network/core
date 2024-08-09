use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::Borsh;
use crate::key;
use crate::types::PredefinedEntry;

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct ApplicationMeta {
    // todo! impl proper entry reference count
    // pub refs: usize,
    pub blob: key::BlobMeta,
    pub version: Option<Box<str>>,
    pub source: Box<str>,
    // contract_app_id - Application ID is hash(owner,name) saved in package manager contract
    pub contract_app_id: Option<Box<str>>,
}

impl PredefinedEntry for key::ApplicationMeta {
    type Codec = Borsh;
    type DataType<'a> = ApplicationMeta;
}
