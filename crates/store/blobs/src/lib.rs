use core::fmt::{self, Debug, Formatter};
use core::pin::Pin;
use core::task::{Context, Poll};
use std::io::ErrorKind as IoErrorKind;

use async_stream::try_stream;
use calimero_primitives::blobs::BlobId;
use calimero_store::key::BlobMeta as BlobMetaKey;
use calimero_store::types::BlobMeta;
use calimero_store::Store as DataStore;
use camino::{Utf8Path, Utf8PathBuf};
use eyre::{Report, Result as EyreResult};
use futures_util::{pin_mut, Stream, StreamExt, TryStreamExt};
use sha2::{Digest, Sha256};
use thiserror::Error as ThisError;
use tokio::fs::{create_dir_all, read as async_read, try_exists, write as async_write};

const CHUNK_SIZE: usize = 1 << 18; // 256 KiB

// const MAX_LINKS_PER_BLOB: usize = 256;

#[derive(Clone, Debug)]
pub struct BlobManager {
    data_store: DataStore,
    blob_store: FileSystem, // Arc<dyn BlobRepository>
}

impl BlobManager {
    #[must_use]
    pub const fn new(data_store: DataStore, blob_store: FileSystem) -> Self {
        Self {
            data_store,
            blob_store,
        }
    }

    pub fn has(&self, id: BlobId) -> EyreResult<bool> {
        Ok(self.data_store.handle().has(&BlobMetaKey::new(id))?)
    }

    // return a concrete type that resolves to the content of the file
    pub fn get(&self, id: BlobId) -> EyreResult<Option<Blob>> {
        Blob::new(id, self.clone())
    }

    pub async fn put<S, T, E>(&self, stream: S) -> EyreResult<BlobId>
    where
        // todo! change this to AsyncRead
        S: Stream<Item = Result<T, E>>,
        T: AsRef<[u8]>,
        E: Into<Report>,
    {
        let chunks = typed_stream::<EyreResult<_>>(try_stream!({
            pin_mut!(stream);

            // todo! use a bufreader
            while let Some(blob) = stream.try_next().await? {
                let blob = blob.as_ref();

                for chunk in blob.chunks(CHUNK_SIZE) {
                    let id = BlobId::hash(chunk);

                    self.data_store.handle().put(
                        &BlobMetaKey::new(id),
                        &BlobMeta::new(0, Vec::new().into_boxed_slice()),
                    )?;
                    self.blob_store.put(id, chunk).await?;

                    yield id;
                }

                // let mut chunks = blob
                //     .chunks(CHUNK_SIZE)
                //     .map(|chunk| self.blob_store.put(BlobId::hash(&chunk), chunk))
                //     .collect::<FuturesUnordered<_>>();

                // while let Some(_) = chunks.try_next().await? {}
            }
        }));

        // let ids = ids.chunks(MAX_LINKS_PER_BLOB);
        pin_mut!(chunks);

        let mut links = Vec::new();
        let mut digest = Sha256::new();

        while let Some(id) = chunks.try_next().await? {
            links.push(BlobMetaKey::new(id));
            digest.update(id.as_ref());
        }

        let id = BlobId::from(*(AsRef::<[u8; 32]>::as_ref(&digest.finalize())));

        self.data_store.handle().put(
            &BlobMetaKey::new(id),
            &BlobMeta::new(
                0,
                links.into_boxed_slice(),
                // todo! hash of the blob data
            ),
        )?;

        Ok(id) // todo!: Ok((id, Blob { size, hash }::{fn stream()}))
    }
}

fn typed_stream<T>(s: impl Stream<Item = T>) -> impl Stream<Item = T> {
    s
}

pub struct Blob {
    // id: BlobId,
    // meta: BlobMeta,

    // blob_mgr: BlobManager,
    #[allow(clippy::type_complexity)]
    stream: Pin<Box<dyn Stream<Item = Result<Box<[u8]>, BlobError>>>>,
}

impl Blob {
    fn new(id: BlobId, blob_mgr: BlobManager) -> EyreResult<Option<Self>> {
        let Some(blob_meta) = blob_mgr.data_store.handle().get(&BlobMetaKey::new(id))? else {
            return Ok(None);
        };

        #[allow(clippy::semicolon_if_nothing_returned)]
        let stream = Box::pin(try_stream!({
            if blob_meta.links.is_empty() {
                let maybe_blob = blob_mgr.blob_store.get(id).await;
                let maybe_blob = maybe_blob.map_err(BlobError::RepoError)?;
                let blob = maybe_blob.ok_or_else(|| BlobError::DanglingBlob { id })?;
                return yield blob;
            }

            for link in blob_meta.links {
                let maybe_link = Self::new(link.blob_id(), blob_mgr.clone());
                let maybe_link = maybe_link.map_err(BlobError::RepoError)?;
                let link = maybe_link.ok_or_else(|| BlobError::DanglingBlob { id })?;
                for await blob in link {
                    yield blob?;
                }
            }
        }));

        Ok(Some(Self { stream }))
    }
}

impl Debug for Blob {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        // TODO: Add more details if/when additional fields are added to Blob
        f.debug_struct("Blob").finish()
    }
}

#[derive(Debug, ThisError)]
#[allow(variant_size_differences)]
#[non_exhaustive]
pub enum BlobError {
    #[error("encountered a dangling Blob ID: `{id}`, the blob store may be corrupt")]
    DanglingBlob { id: BlobId },
    #[error(transparent)]
    RepoError(Report),
}

impl Stream for Blob {
    type Item = Result<Box<[u8]>, BlobError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.poll_next_unpin(cx)
    }
}

trait BlobRepository {
    #[allow(dead_code)]
    async fn has(&self, id: BlobId) -> EyreResult<bool>;
    async fn get(&self, id: BlobId) -> EyreResult<Option<Box<[u8]>>>;
    async fn put(&self, id: BlobId, data: &[u8]) -> EyreResult<()>;
}

#[derive(Clone, Debug)]
pub struct FileSystem {
    root: Utf8PathBuf,
    // strategy: ShardingStrategy,
}

// enum ShardingStrategy {
//     NextToLast(Tolerance)
// }

impl FileSystem {
    pub async fn new(root: &Utf8Path) -> EyreResult<Self> {
        create_dir_all(&root).await?;

        Ok(Self {
            root: root.to_owned(),
        })
    }

    fn path(&self, id: BlobId) -> Utf8PathBuf {
        self.root.join(id.as_str())
    }
}

impl BlobRepository for FileSystem {
    async fn has(&self, id: BlobId) -> EyreResult<bool> {
        try_exists(self.path(id)).await.map_err(Into::into)
    }

    async fn get(&self, id: BlobId) -> EyreResult<Option<Box<[u8]>>> {
        match async_read(self.path(id)).await {
            Ok(file) => Ok(Some(file.into_boxed_slice())),
            Err(err) if err.kind() == IoErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    async fn put(&self, id: BlobId, data: &[u8]) -> EyreResult<()> {
        async_write(self.path(id), data).await.map_err(Into::into)
    }
}
