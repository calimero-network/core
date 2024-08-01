use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::Borsh;
use crate::key;
use crate::types::PredefinedEntry;

#[derive(Eq, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct BlobMeta {
    // todo! impl proper entry reference count
    pub refs: usize,
    pub size: usize,
    pub links: Box<[key::BlobMeta]>,
}

impl PredefinedEntry for key::BlobMeta {
    type Codec = Borsh;
    type DataType<'a> = BlobMeta;
}
