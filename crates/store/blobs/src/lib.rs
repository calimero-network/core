use core::fmt::{self, Debug, Formatter};
use core::pin::Pin;
use core::task::{Context, Poll};
use std::io::ErrorKind as IoErrorKind;

use async_stream::try_stream;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::hash::Hash;
use calimero_store::key::BlobMeta as BlobMetaKey;
use calimero_store::types::BlobMeta;
use calimero_store::Store as DataStore;
use camino::{Utf8Path, Utf8PathBuf};
use eyre::{Report, Result as EyreResult};
use futures_util::io::BufReader;
use futures_util::{pin_mut, AsyncRead, AsyncReadExt, Stream, StreamExt, TryStreamExt};
use sha2::{Digest, Sha256};
use thiserror::Error as ThisError;
use tokio::fs::{create_dir_all, read as async_read, try_exists, write as async_write};

const CHUNK_SIZE: usize = 1 << 20; // 1MiB

// const MAX_LINKS_PER_BLOB: usize = 128;

#[derive(Clone, Debug)]
pub struct BlobManager {
    data_store: DataStore,
    blob_store: FileSystem, // Arc<dyn BlobRepository>
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Value {
    Full { hash: Hash, size: usize },
    Part { id: BlobId, _size: usize },
}

#[derive(Clone, Debug, Default)]
struct State {
    digest: Sha256,
    size: usize,
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

    pub async fn put<T>(&self, stream: T) -> EyreResult<BlobId>
    where
        T: AsyncRead,
    {
        self.put_sized(None, stream).await
    }

    pub async fn put_sized<T>(&self, size_hint: Option<u64>, stream: T) -> EyreResult<BlobId>
    where
        T: AsyncRead,
    {
        let stream = BufReader::new(stream);

        pin_mut!(stream);

        let blobs = try_stream!({
            let mut buf = vec![0_u8; CHUNK_SIZE].into_boxed_slice();
            let mut file = State::default();
            let mut blob = State::default();

            loop {
                let bytes = stream.read(&mut buf[blob.size..]).await?;

                let finished = bytes == 0;

                if !finished {
                    let chunk = &buf[blob.size..blob.size.saturating_add(bytes)];

                    file.digest.update(chunk);
                    blob.digest.update(chunk);

                    blob.size = blob.size.saturating_add(bytes);

                    if blob.size != buf.len() {
                        continue;
                    }
                }

                if blob.size == 0 {
                    break;
                }

                let id = BlobId::from(*AsRef::<[u8; 32]>::as_ref(&blob.digest.finalize()));

                self.data_store.handle().put(
                    &BlobMetaKey::new(id),
                    &BlobMeta::new(blob.size, *id, Box::default()),
                )?;

                self.blob_store.put(id, &buf[..blob.size]).await?;

                file.size = file.size.saturating_add(blob.size);

                yield Value::Part {
                    id,
                    _size: blob.size,
                };

                if finished {
                    break;
                }

                blob = State::default();
            }

            yield Value::Full {
                hash: Hash::from(*(AsRef::<[u8; 32]>::as_ref(&file.digest.finalize()))),
                size: file.size,
            };
        });

        let blobs = typed_stream::<EyreResult<_>>(blobs).peekable();
        pin_mut!(blobs);

        let mut links = Vec::with_capacity(
            size_hint
                .and_then(|s| {
                    usize::try_from(s)
                        .map(|s| s.saturating_div(CHUNK_SIZE))
                        .ok()
                })
                .unwrap_or_default(),
        );
        let mut digest = Sha256::new();

        while let Some(Value::Part { id, _size }) = blobs
            .as_mut()
            .next_if(|v| matches!(v, Ok(Value::Part { .. })))
            .await
            .transpose()?
        {
            links.push(BlobMetaKey::new(id));
            digest.update(id.as_ref());
        }

        let Some(Value::Full { hash, size }) = blobs.try_next().await? else {
            unreachable!("the root should always be emitted");
        };

        let id = BlobId::from(*(AsRef::<[u8; 32]>::as_ref(&digest.finalize())));

        self.data_store.handle().put(
            &BlobMetaKey::new(id),
            &BlobMeta::new(size, *hash, links.into_boxed_slice()),
        )?;

        Ok(id) // todo!: Ok(Blob { id, size, hash }::{fn stream()})
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

#[cfg(test)]
mod integration_tests_package_usage {
    use tokio_util as _;
}
