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

impl PredefinedEntry for key::BlobOwner {
    type Codec = Borsh;
    type DataType<'a> = ();
}

impl PredefinedEntry for key::PrefetchedBlob {
    type Codec = Borsh;
    type DataType<'a> = ();
}

/// One frontend file of a bundle: its archive path and the blob holding its
/// bytes. The blob id is content-addressed, so it doubles as a strong ETag.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct FrontendFile {
    pub path: Box<str>,
    pub blob: key::BlobMeta,
}

impl FrontendFile {
    #[must_use]
    pub const fn new(path: Box<str>, blob: key::BlobMeta) -> Self {
        Self { path, blob }
    }
}

/// Value for [`key::BundleFrontend`]: the owning application, the release it
/// belongs to, the entry path and every stored file. A brand-new key, so a node that never stored one reads `None`.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct BundleFrontend {
    pub application: key::ApplicationMeta,
    pub version: Box<str>,
    pub entry: Box<str>,
    pub files: Vec<FrontendFile>,
}

impl BundleFrontend {
    #[must_use]
    pub const fn new(
        application: key::ApplicationMeta,
        version: Box<str>,
        entry: Box<str>,
        files: Vec<FrontendFile>,
    ) -> Self {
        Self {
            application,
            version,
            entry,
            files,
        }
    }
}

impl PredefinedEntry for key::BundleFrontend {
    type Codec = Borsh;
    type DataType<'a> = BundleFrontend;
}

#[cfg(test)]
mod bundle_frontend_tests {
    use std::sync::Arc;

    use calimero_primitives::blobs::BlobId;

    use super::{BundleFrontend, FrontendFile};
    use crate::db::InMemoryDB;
    use crate::{key, Store};

    #[test]
    fn an_absent_key_reads_as_none() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let bundle = BlobId::from([7; 32]);

        let read = store
            .handle()
            .get(&key::BundleFrontend::new(bundle))
            .expect("read an absent key");

        assert_eq!(read, None);
    }

    #[test]
    fn a_written_index_reads_back() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let bundle = BlobId::from([7; 32]);
        let index = BundleFrontend::new(
            key::ApplicationMeta::new([3; 32].into()),
            "1.0.0".into(),
            "frontend/index.html".into(),
            vec![FrontendFile::new(
                "frontend/index.html".into(),
                key::BlobMeta::new(BlobId::from([1; 32])),
            )],
        );

        let mut handle = store.handle();
        handle
            .put(&key::BundleFrontend::new(bundle), &index)
            .unwrap();

        assert_eq!(
            handle.get(&key::BundleFrontend::new(bundle)).unwrap(),
            Some(index)
        );
    }
}
