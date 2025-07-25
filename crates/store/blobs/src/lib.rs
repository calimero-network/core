use core::fmt::{self, Debug, Formatter};
use core::pin::{pin, Pin};
use core::task::{Context, Poll};
use std::io::ErrorKind as IoErrorKind;

use async_stream::try_stream;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::hash::Hash;
use calimero_store::key::BlobMeta as BlobMetaKey;
use calimero_store::types::BlobMeta as BlobMetaValue;
use calimero_store::Store as DataStore;
use camino::Utf8PathBuf;
use eyre::{Report, Result as EyreResult};
use futures_util::io::BufReader;
use futures_util::{AsyncRead, AsyncReadExt, Stream, StreamExt, TryStreamExt};
use sha2::{Digest, Sha256};
use thiserror::Error as ThisError;
use tokio::fs::{create_dir_all, read as async_read, try_exists, write as async_write};

pub mod config;

use config::BlobStoreConfig;

pub const CHUNK_SIZE: usize = 1 << 20; // 1MiB
const _: [(); { (usize::BITS - CHUNK_SIZE.leading_zeros()) > 32 } as usize] = [
    /* CHUNK_SIZE must be a 32-bit number */
];

// const MAX_LINKS_PER_BLOB: usize = 128;

#[derive(Clone, Debug)]
pub struct BlobManager {
    data_store: DataStore,
    blob_store: FileSystem, // Arc<dyn BlobRepository>
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Value {
    Full { hash: Hash, size: u64 },
    Part { id: BlobId, _size: u64 },
    Overflow { found: u64, expected: u64 },
}

#[derive(Clone, Debug, Default)]
struct State {
    digest: Sha256,
    size: usize,
}

#[expect(clippy::exhaustive_enums, reason = "There are no more variants to add")]
#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd)]
pub enum Size {
    Hint(u64),
    Exact(u64),
}

impl Size {
    const fn hint(&self) -> usize {
        // TODO: Check this, as the incoming int is a u64
        #[expect(
            clippy::cast_possible_truncation,
            reason = "This is never expected to overflow"
        )]
        match self {
            Self::Hint(size) | Self::Exact(size) => *size as usize,
        }
    }

    fn overflows(this: Option<&Self>, size: usize) -> Option<u64> {
        let size = u64::try_from(size);

        match this {
            None | Some(Self::Hint(_)) => size.err().map(|_| u64::MAX),
            Some(Self::Exact(exact)) => {
                size.map_or_else(|_| Some(*exact), |s| (s > *exact).then_some(*exact))
            }
        }
    }
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

    pub async fn delete(&self, id: BlobId) -> EyreResult<bool> {
        self.blob_store.delete(id).await
    }

    pub async fn put<T>(&self, stream: T) -> EyreResult<(BlobId, Hash, u64)>
    where
        T: AsyncRead,
    {
        self.put_sized(None, stream).await
    }

    pub async fn put_sized<T>(
        &self,
        size: Option<Size>,
        stream: T,
    ) -> EyreResult<(BlobId, Hash, u64)>
    where
        T: AsyncRead,
    {
        let mut stream = pin!(BufReader::new(stream));

        let blobs = try_stream!({
            let mut buf = vec![0_u8; CHUNK_SIZE].into_boxed_slice();
            let mut file = State::default();
            let mut blob = State::default();

            let overflow_data = loop {
                let bytes = stream.read(&mut buf[blob.size..]).await?;

                let finished = bytes == 0;

                if !finished {
                    let new_blob_size = blob.size.saturating_add(bytes);
                    let new_file_size = file.size.saturating_add(bytes);

                    let chunk = &buf[blob.size..new_blob_size];

                    blob.size = new_blob_size;
                    file.size = new_file_size;

                    if let Some(expected) = Size::overflows(size.as_ref(), new_file_size) {
                        break Some(expected);
                    }

                    blob.digest.update(chunk);
                    file.digest.update(chunk);

                    if blob.size != buf.len() {
                        continue;
                    }
                }

                if blob.size == 0 {
                    break None;
                }

                let id = BlobId::from(*AsRef::<[u8; 32]>::as_ref(&blob.digest.finalize()));

                self.data_store.handle().put(
                    &BlobMetaKey::new(id),
                    &BlobMetaValue::new(blob.size as u64, *id, Box::default()),
                )?;

                self.blob_store.put(id, &buf[..blob.size]).await?;

                yield Value::Part {
                    id,
                    _size: blob.size as u64,
                };

                if finished {
                    break None;
                }

                blob = State::default();
            };

            if let Some(expected) = overflow_data {
                yield Value::Overflow {
                    found: file.size as u64,
                    expected,
                };
            } else {
                yield Value::Full {
                    hash: Hash::from(*(AsRef::<[u8; 32]>::as_ref(&file.digest.finalize()))),
                    size: file.size as u64,
                };
            }
        });

        let blobs = typed_stream::<EyreResult<_>>(blobs).peekable();
        let mut blobs = pin!(blobs);

        let mut links = Vec::with_capacity(
            size.map(|s| s.hint().saturating_div(CHUNK_SIZE))
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

        let (hash, size) = match blobs.try_next().await? {
            Some(Value::Full { hash, size }) => (hash, size),
            Some(Value::Overflow { found, expected }) => {
                eyre::bail!("expected {} bytes in the stream, found {}", expected, found)
            }
            _ => {
                unreachable!("the root should always be emitted");
            }
        };

        let id = BlobId::from(*(AsRef::<[u8; 32]>::as_ref(&digest.finalize())));

        self.data_store.handle().put(
            &BlobMetaKey::new(id),
            &BlobMetaValue::new(size, *hash, links.into_boxed_slice()),
        )?;

        Ok((id, hash, size)) // todo!: Ok(Blob { id, size, hash }::{fn stream()})
    }
}

fn typed_stream<T>(s: impl Stream<Item = T>) -> impl Stream<Item = T> {
    s
}

pub struct Blob {
    // id: BlobId,
    // meta: BlobMeta,

    // blob_mgr: BlobManager,
    #[expect(clippy::type_complexity, reason = "Acceptable here")]
    stream: Pin<Box<dyn Stream<Item = Result<Box<[u8]>, BlobError>> + Send>>,
}

impl Blob {
    fn new(id: BlobId, blob_mgr: BlobManager) -> EyreResult<Option<Self>> {
        let Some(blob_meta) = blob_mgr.data_store.handle().get(&BlobMetaKey::new(id))? else {
            return Ok(None);
        };

        #[expect(
            clippy::semicolon_if_nothing_returned,
            reason = "False positive; not possible with macro"
        )]
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
#[expect(variant_size_differences, reason = "Doesn't matter here")]
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
    #[expect(dead_code, reason = "Will be used in future")]
    async fn has(&self, id: BlobId) -> EyreResult<bool>;
    async fn get(&self, id: BlobId) -> EyreResult<Option<Box<[u8]>>>;
    async fn put(&self, id: BlobId, data: &[u8]) -> EyreResult<()>;
    async fn delete(&self, id: BlobId) -> EyreResult<bool>;
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
    pub async fn new(config: &BlobStoreConfig) -> EyreResult<Self> {
        create_dir_all(&config.path).await?;

        Ok(Self {
            root: config.path.clone(),
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

    async fn delete(&self, id: BlobId) -> EyreResult<bool> {
        let path = self.path(id);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == IoErrorKind::NotFound => Ok(false),
            Err(err) => Err(err.into()),
        }
    }
}

#[cfg(test)]
mod integration_tests_package_usage {
    use tokio_util as _;
}
