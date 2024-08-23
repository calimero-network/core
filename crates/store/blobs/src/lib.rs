use std::pin::Pin;
use std::task::{Context, Poll};

use async_stream::try_stream;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::hash::Hash;
use calimero_store::Store as DataStore;
use futures_util::io::BufReader;
use futures_util::{pin_mut, AsyncRead, AsyncReadExt, Stream, StreamExt, TryStreamExt};
use sha2::Digest;
use thiserror::Error;
use tokio::fs;

const CHUNK_SIZE: usize = 1 << 20; // 1MiB

// const MAX_LINKS_PER_BLOB: usize = 128;

#[derive(Clone)]
pub struct BlobManager {
    data_store: DataStore,
    blob_store: FileSystem, // Arc<dyn BlobRepository>
}

#[derive(Debug)]
enum Value {
    Full { hash: Hash, size: usize },
    Part { id: BlobId, _size: usize },
}

#[derive(Default)]
struct State {
    digest: sha2::Sha256,
    size: usize,
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

    pub async fn put<T>(&self, stream: T) -> eyre::Result<BlobId>
    where
        T: AsyncRead,
    {
        self.put_sized(None, stream).await
    }

    pub async fn put_sized<T>(&self, size_hint: Option<u64>, stream: T) -> eyre::Result<BlobId>
    where
        T: AsyncRead,
    {
        let stream = BufReader::new(stream);

        pin_mut!(stream);

        let blobs = try_stream!({
            let mut buf = {
                let mut buf = Vec::with_capacity(CHUNK_SIZE);
                unsafe { buf.set_len(CHUNK_SIZE) };
                buf.into_boxed_slice()
            };

            let mut file = State::default();
            let mut blob = State::default();

            loop {
                let bytes = stream.read(&mut buf[blob.size..]).await?;

                let finished = bytes == 0;

                if !finished {
                    let chunk = &buf[blob.size..blob.size + bytes];

                    file.digest.update(chunk);
                    blob.digest.update(chunk);

                    blob.size += bytes;

                    if blob.size != buf.len() {
                        continue;
                    }
                }

                if blob.size == 0 {
                    break;
                }

                let id = BlobId::from(*AsRef::<[u8; 32]>::as_ref(&blob.digest.finalize()));

                self.data_store.handle().put(
                    &calimero_store::key::BlobMeta::new(id),
                    &calimero_store::types::BlobMeta {
                        size: blob.size,
                        hash: *id,
                        links: Default::default(),
                    },
                )?;

                self.blob_store.put(id, &buf[..blob.size]).await?;

                file.size += blob.size;

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

        let blobs = typed_stream::<eyre::Result<_>>(blobs).peekable();
        pin_mut!(blobs);

        let mut links = Vec::with_capacity(
            size_hint
                .and_then(|s| usize::try_from(s).map(|s| s / CHUNK_SIZE).ok())
                .unwrap_or_default(),
        );
        let mut digest = sha2::Sha256::new();

        while let Some(Value::Part { id, _size }) = blobs
            .as_mut()
            .next_if(|v| matches!(v, Ok(Value::Part { .. })))
            .await
            .transpose()?
        {
            links.push(calimero_store::key::BlobMeta::new(id));
            digest.update(id.as_ref());
        }

        let Some(Value::Full { hash, size }) = blobs.try_next().await? else {
            unreachable!("the root should always be emitted");
        };

        let id = BlobId::from(*(AsRef::<[u8; 32]>::as_ref(&digest.finalize())));

        self.data_store.handle().put(
            &calimero_store::key::BlobMeta::new(id),
            &calimero_store::types::BlobMeta {
                size,
                hash: *hash,
                links: links.into_boxed_slice(),
            },
        )?;

        Ok(id) // todo!: Ok(Blob { id, size, hash }::{fn stream()})
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

#[derive(Debug, Error)]
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

#[derive(Clone)]
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
