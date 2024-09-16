use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::Borsh;
use crate::key::{ApplicationMeta as ApplicationMetaKey, BlobMeta as BlobMetaKey};
use crate::types::PredefinedEntry;

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ApplicationMeta {
    // todo! impl proper entry reference count
    // pub refs: usize,
    pub blob: BlobMetaKey,
    pub source: Box<str>,
    pub metadata: Box<[u8]>,
}

impl ApplicationMeta {
    #[must_use]
    pub fn new(blob: BlobMetaKey, source: Box<str>, metadata: Box<[u8]>) -> Self {
        Self {
            blob,
            source,
            metadata,
        }
    }
}

impl PredefinedEntry for ApplicationMetaKey {
    type Codec = Borsh;
    type DataType<'a> = ApplicationMeta;
}
