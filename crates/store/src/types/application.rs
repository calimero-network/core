use borsh::{BorshDeserialize, BorshSerialize};

use crate::entry::Borsh;
use crate::key;
use crate::types::PredefinedEntry;

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ApplicationMeta {
    // todo! impl proper entry reference count
    // pub refs: usize,
    pub bytecode: key::BlobMeta,
    pub size: u64,
    pub source: Box<str>,    // todo! use Cow<'a, str>
    pub metadata: Box<[u8]>, // todo! use Cow<'a, [u8]>
    pub compiled: key::BlobMeta,
}

impl ApplicationMeta {
    #[must_use]
    pub const fn new(
        bytecode: key::BlobMeta,
        size: u64,
        source: Box<str>,
        metadata: Box<[u8]>,
        compiled: key::BlobMeta,
    ) -> Self {
        Self {
            bytecode,
            size,
            source,
            metadata,
            compiled,
        }
    }
}

impl PredefinedEntry for key::ApplicationMeta {
    type Codec = Borsh;
    type DataType<'a> = ApplicationMeta;
}
