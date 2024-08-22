use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::Borsh;
use crate::key;
use crate::types::PredefinedEntry;

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ApplicationMeta {
    // todo! impl proper entry reference count
    // pub refs: usize,
    pub blob: key::BlobMeta,
    pub version: Option<Box<str>>,
    pub source: Box<str>,
    pub metadata: Box<[u8]>,
}

impl ApplicationMeta {
    #[must_use]
    pub fn new(
        blob: key::BlobMeta,
        version: Option<Box<str>>,
        source: Box<str>,
        metadata: Box<[u8]>,
    ) -> Self {
        Self {
            blob,
            version,
            source,
            metadata,
        }
    }
}

impl PredefinedEntry for key::ApplicationMeta {
    type Codec = Borsh;
    type DataType<'a> = ApplicationMeta;
}
