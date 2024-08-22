use std::fmt::{self, Debug, Formatter};
use std::pin::Pin;
use std::task::{Context, Poll};

use async_stream::try_stream;
use calimero_primitives::blobs::BlobId;
use calimero_store::Store as DataStore;
use futures_util::{pin_mut, Stream, StreamExt, TryStreamExt};
use sha2::Digest;
use thiserror::Error;
use tokio::fs;

const CHUNK_SIZE: usize = 1 << 18; // 256 KiB

// const MAX_LINKS_PER_BLOB: usize = 256;

#[derive(Clone, Debug)]
pub struct BlobManager {
    data_store: DataStore,
    blob_store: FileSystem, // Arc<dyn BlobRepository>
}

impl BlobManager {
    pub fn new(data_store: DataStore, blob_store: FileSystem) -> Self {
        BlobManager {
            data_store,
            blob_store,
        }
    }

    pub async fn has(&self, id: BlobId) -> eyre::Result<bool> {
        Ok(self
            .data_store
            .handle()
            .has(&calimero_store::key::BlobMeta::new(id))?)
    }

    // return a concrete type that resolves to the content of the file
    pub async fn get(&self, id: BlobId) -> eyre::Result<Option<Blob>> {
        Blob::new(id, self.clone()).await
    }

    pub async fn put<S, T, E>(&self, stream: S) -> eyre::Result<BlobId>
    where
        // todo! change this to AsyncRead
        S: Stream<Item = Result<T, E>>,
        T: AsRef<[u8]>,
        E: Into<eyre::Report>,
    {
        let chunks = typed_stream::<eyre::Result<_>>(try_stream!({
            pin_mut!(stream);

            // todo! use a bufreader
            while let Some(blob) = stream.try_next().await? {
                let blob = blob.as_ref();

                for chunk in blob.chunks(CHUNK_SIZE) {
                    let id = BlobId::hash(&chunk);

                    self.data_store.handle().put(
                        &calimero_store::key::BlobMeta::new(id),
                        &calimero_store::types::BlobMeta {
                            size: 0,
                            links: Vec::new().into_boxed_slice(),
                        },
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
        let mut digest = sha2::Sha256::new();

        while let Some(id) = chunks.try_next().await? {
            links.push(calimero_store::key::BlobMeta::new(id));
            digest.update(id.as_ref());
        }

        let id = BlobId::from(*(AsRef::<[u8; 32]>::as_ref(&digest.finalize())));

        self.data_store.handle().put(
            &calimero_store::key::BlobMeta::new(id),
            &calimero_store::types::BlobMeta {
                size: 0,
                links: links.into_boxed_slice(),
                // todo! hash of the blob data
            },
        )?;

        Ok(id) // todo!: Ok((id, Blob { size, hash }::{fn stream()}))
    }
}

fn typed_stream<T>(s: impl Stream<Item = T>) -> impl Stream<Item = T> {
    s
}

pub struct Blob {
    // id: BlobId,
    // meta: calimero_store::types::BlobMeta,

    // blob_mgr: BlobManager,
    stream: Pin<Box<dyn Stream<Item = Result<Box<[u8]>, BlobError>>>>,
}

impl Blob {
    async fn new(id: BlobId, blob_mgr: BlobManager) -> eyre::Result<Option<Self>> {
        let Some(blob_meta) = blob_mgr
            .data_store
            .handle()
            .get(&calimero_store::key::BlobMeta::new(id))?
        else {
            return Ok(None);
        };

        let stream = Box::pin(try_stream!({
            if blob_meta.links.is_empty() {
                let maybe_blob = blob_mgr.blob_store.get(id).await;
                let maybe_blob = maybe_blob.map_err(BlobError::RepoError)?;
                let blob = maybe_blob.ok_or_else(|| BlobError::DanglingBlob { id })?;
                return yield blob;
            }

            for link in blob_meta.links {
                let maybe_link = Blob::new(link.blob_id(), blob_mgr.clone()).await;
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

#[derive(Debug, Error)]
#[allow(variant_size_differences)]
pub enum BlobError {
    #[error("encountered a dangling Blob ID: `{id}`, the blob store may be corrupt")]
    DanglingBlob { id: BlobId },
    #[error(transparent)]
    RepoError(eyre::Report),
}

impl Stream for Blob {
    type Item = Result<Box<[u8]>, BlobError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.poll_next_unpin(cx)
    }
}

trait BlobRepository {
    #[allow(dead_code)]
    async fn has(&self, id: BlobId) -> eyre::Result<bool>;
    async fn get(&self, id: BlobId) -> eyre::Result<Option<Box<[u8]>>>;
    async fn put(&self, id: BlobId, data: &[u8]) -> eyre::Result<()>;
}

#[derive(Clone, Debug)]
pub struct FileSystem {
    root: camino::Utf8PathBuf,
    // strategy: ShardingStrategy,
}

// enum ShardingStrategy {
//     NextToLast(Tolerance)
// }

impl FileSystem {
    pub async fn new(root: &camino::Utf8Path) -> eyre::Result<Self> {
        fs::create_dir_all(&root).await?;

        Ok(Self {
            root: root.to_owned(),
        })
    }

    fn path(&self, id: BlobId) -> camino::Utf8PathBuf {
        self.root.join(id.as_str())
    }
}

impl BlobRepository for FileSystem {
    async fn has(&self, id: BlobId) -> eyre::Result<bool> {
        fs::try_exists(self.path(id)).await.map_err(Into::into)
    }

    async fn get(&self, id: BlobId) -> eyre::Result<Option<Box<[u8]>>> {
        match fs::read(self.path(id)).await {
            Ok(file) => Ok(Some(file.into_boxed_slice())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    async fn put(&self, id: BlobId, data: &[u8]) -> eyre::Result<()> {
        fs::write(self.path(id), data).await.map_err(Into::into)
    }
}
