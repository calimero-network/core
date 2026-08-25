use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::content_hash::ContentHash;

use crate::entry::Borsh;
use crate::key;
use crate::types::PredefinedEntry;

#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct BlobMeta {
    pub size: u64,
    pub content_hash: ContentHash,
    pub links: Box<[key::BlobMeta]>,
    /// Number of live references to this content-addressed blob.
    ///
    /// Blobs are deduplicated by content hash: a given byte sequence is stored
    /// once but may be referenced by many owners — the same bytes added by
    /// several contexts, or a chunk shared by several root blobs. Each add
    /// increments this count and each delete decrements it; the backing file
    /// and this row are removed only when it reaches zero. Without it, one
    /// owner deleting a blob would destroy content another still references.
    pub refs: u32,
}

impl BlobMeta {
    #[must_use]
    pub const fn new(
        size: u64,
        content_hash: ContentHash,
        links: Box<[key::BlobMeta]>,
        refs: u32,
    ) -> Self {
        Self {
            size,
            content_hash,
            links,
            refs,
        }
    }
}

impl PredefinedEntry for key::BlobMeta {
    type Codec = Borsh;
    type DataType<'a> = BlobMeta;
}
