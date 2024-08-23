use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::Borsh;
use crate::key::BlobMeta as BlobMetaKey;
use crate::types::PredefinedEntry;

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct BlobMeta {
    // todo! impl proper entry reference count
    // pub refs: usize,
    pub size: usize,
    pub links: Box<[BlobMetaKey]>,
}

impl BlobMeta {
    #[must_use]
    pub const fn new(size: usize, links: Box<[BlobMetaKey]>) -> Self {
        Self { size, links }
    }
}

impl PredefinedEntry for BlobMetaKey {
    type Codec = Borsh;
    type DataType<'a> = BlobMeta;
}
