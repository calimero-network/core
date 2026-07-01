use core::fmt::{self, Debug, Formatter};
use core::pin::{pin, Pin};
use core::task::{Context, Poll};
use std::collections::HashSet;
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
use tracing::{debug, error, trace};

pub mod config;
mod utils;

use config::BlobStoreConfig;

pub const CHUNK_SIZE: usize = 1 << 20; // 1MiB
const _: [(); { (usize::BITS - CHUNK_SIZE.leading_zeros()) > 32 } as usize] = [
    /* CHUNK_SIZE must be a 32-bit number */
];

/// Hard bounds on a blob's meta-graph traversal. The graph is persisted and
/// synced from peers, so it is untrusted input: a deeply-nested chain would
/// overflow the stack under the old recursive walk, and a back-edge (cycle)
/// would loop forever. `put` only ever produces a shallow tree (root → leaf
/// parts), so these caps are far above any legitimate graph and only trip on
/// corrupt or malicious meta.
const MAX_BLOB_DEPTH: usize = 64;
const MAX_BLOB_NODES: usize = 1 << 20;

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

    /// Get the package directory path
    ///
    /// # Errors
    /// Returns an error if `package` is not a safe path component.
    pub fn package_path(&self, package: &str) -> EyreResult<Utf8PathBuf> {
        self.blob_store.package_path(package)
    }

    /// Get the version directory path
    ///
    /// # Errors
    /// Returns an error if `package` or `version` is not a safe path component.
    pub fn version_path(&self, package: &str, version: &str) -> EyreResult<Utf8PathBuf> {
        self.blob_store.version_path(package, version)
    }

    /// Get the root/base path of the blobstore
    pub fn root_path(&self) -> Utf8PathBuf {
        self.blob_store.root_path()
    }

    /// Get the path for a blob stored in a package/version directory
    ///
    /// # Errors
    /// Returns an error if `package` or `version` is not a safe path component.
    pub fn application_blob_path(
        &self,
        package: &str,
        version: &str,
        id: BlobId,
    ) -> EyreResult<Utf8PathBuf> {
        self.blob_store.application_blob_path(package, version, id)
    }

    pub fn has(&self, id: BlobId) -> EyreResult<bool> {
        Ok(self.data_store.handle().has(&BlobMetaKey::new(id))?)
    }

    // return a concrete type that resolves to the content of the file
    pub fn get(&self, id: BlobId) -> EyreResult<Option<Blob>> {
        Blob::new(id, self.clone())
    }

    pub async fn delete(&self, id: BlobId) -> EyreResult<bool> {
        let key = BlobMetaKey::new(id);

        // Remove the metadata row alongside the chunk file. Without this, `has`
        // keeps reporting the blob as present (it only checks metadata) while
        // `get` fails because the underlying file is gone.
        let had_meta = self.data_store.handle().has(&key)?;
        self.data_store.handle().delete(&key)?;

        let had_file = self.blob_store.delete(id).await?;

        Ok(had_meta || had_file)
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
        debug!(
            size_hint = size.as_ref().map(Size::hint),
            "put_sized invoked"
        );

        let mut stream = pin!(BufReader::new(stream));

        let blobs = try_stream!({
            let mut buf = vec![0_u8; CHUNK_SIZE].into_boxed_slice();
            let mut file = State::default();
            let mut blob = State::default();
            let mut chunk_index: u64 = 0;

            let overflow_data = loop {
                let bytes = stream.read(&mut buf[blob.size..]).await?;

                let finished = bytes == 0;

                if !finished {
                    let new_blob_size = blob.size.saturating_add(bytes);
                    let new_file_size = file.size.saturating_add(bytes);

                    let chunk = &buf[blob.size..new_blob_size];

                    blob.size = new_blob_size;
                    file.size = new_file_size;

                    trace!(
                        chunk_index,
                        chunk_bytes = chunk.len(),
                        file_bytes = file.size,
                        "read chunk data from stream"
                    );

                    if let Some(expected) = Size::overflows(size.as_ref(), new_file_size) {
                        trace!(
                            expected,
                            file_bytes = file.size,
                            "size overflow detected while chunking"
                        );
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

                trace!(
                    ?id,
                    chunk_index,
                    chunk_size = blob.size,
                    file_bytes = file.size,
                    "blob chunk persisted"
                );
                chunk_index += 1;

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

        let chunk_count = links.len();

        let (hash, size) = match blobs.try_next().await? {
            Some(Value::Full { hash, size }) => (hash, size),
            Some(Value::Overflow { found, expected }) => {
                error!(
                    found,
                    expected, "blob size overflow while finalising stream"
                );
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

        debug!(
            ?id,
            total_size = size,
            chunk_count,
            "blob metadata persisted"
        );

        debug!(
            ?id,
            total_size = size,
            chunk_count,
            "blob stored successfully"
        );

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
        // Resolve the root meta up front so an unknown blob (`None`) stays
        // distinguishable from a known-but-empty/corrupt one.
        let Some(root_meta) = blob_mgr.data_store.handle().get(&BlobMetaKey::new(id))? else {
            trace!(?id, "blob metadata not found");
            return Ok(None);
        };

        let stream = Box::pin(try_stream!({
            // Iterative pre-order walk of the meta graph. The old version
            // recursed via `Self::new` per link, so a deep chain overflowed the
            // stack and a cycle looped forever. An explicit stack plus a visited
            // set and depth/node budgets bound both while preserving link order.
            let mut visited: HashSet<BlobId> = HashSet::new();
            let mut nodes_seen: usize = 0;
            let mut chunk_index: u64 = 0;

            // Frames carry the node's already-loaded meta so a missing child is
            // caught as `DanglingBlob` when it is enqueued (matching the old
            // recursive behaviour). Children are pushed in reverse so the LIFO
            // stack emits them in link order.
            let mut stack: Vec<(BlobId, BlobMetaValue, usize)> = vec![(id, root_meta, 0)];

            while let Some((node_id, meta, depth)) = stack.pop() {
                nodes_seen += 1;
                if nodes_seen > MAX_BLOB_NODES {
                    error!(?id, nodes_seen, "blob meta graph exceeds node budget");
                    Err(BlobError::CorruptGraph {
                        id,
                        reason: "meta graph exceeds node budget",
                    })?;
                }

                // A DAG can legitimately reach a node twice, but the tree `put`
                // produces never does; a repeat is a back-edge that would loop
                // forever, so reject it.
                if !visited.insert(node_id) {
                    error!(?id, %node_id, "cycle detected in blob meta graph");
                    Err(BlobError::CorruptGraph {
                        id,
                        reason: "meta graph contains a cycle",
                    })?;
                }

                if meta.links.is_empty() {
                    // Leaf: bytes live in the blob store, content-addressed by
                    // the node id. A zero-byte blob legitimately has no stored
                    // file — yield nothing rather than reporting it dangling.
                    if meta.size == 0 {
                        trace!(?id, %node_id, "empty blob, nothing to serve");
                        continue;
                    }

                    let blob = blob_mgr
                        .blob_store
                        .get(node_id)
                        .await
                        .map_err(BlobError::RepoError)?
                        .ok_or(BlobError::DanglingBlob { id: node_id })?;

                    // Re-hash before serving. The chunk on disk (or supplied by a
                    // peer) is untrusted, and a leaf's id IS the sha256 of its
                    // bytes, so a tampered or corrupt chunk fails this check
                    // instead of being served as authentic.
                    let actual = *AsRef::<[u8; 32]>::as_ref(&Sha256::digest(&blob));
                    if actual != *node_id {
                        error!(?id, %node_id, "blob chunk hash mismatch; refusing to serve");
                        Err(BlobError::IntegrityMismatch { id: node_id })?;
                    }

                    trace!(
                        ?id,
                        %node_id,
                        chunk_index,
                        chunk_size = blob.len(),
                        "serving verified blob chunk"
                    );
                    chunk_index += 1;
                    yield blob;
                    continue;
                }

                if depth >= MAX_BLOB_DEPTH {
                    error!(?id, %node_id, depth, "blob meta graph exceeds depth budget");
                    Err(BlobError::CorruptGraph {
                        id,
                        reason: "meta graph exceeds depth budget",
                    })?;
                }

                let mut children: Vec<(BlobId, BlobMetaValue, usize)> =
                    Vec::with_capacity(meta.links.len());
                for link_meta in meta.links.iter() {
                    let child_id = link_meta.blob_id();
                    let child_meta = blob_mgr
                        .data_store
                        .handle()
                        .get(&BlobMetaKey::new(child_id))
                        .map_err(|e| BlobError::RepoError(e.into()))?
                        .ok_or_else(|| {
                            error!(
                                ?id,
                                missing_child = %child_id,
                                "blob metadata missing referenced child"
                            );
                            BlobError::DanglingBlob { id: child_id }
                        })?;
                    children.push((child_id, child_meta, depth + 1));
                }
                for child in children.into_iter().rev() {
                    stack.push(child);
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
#[non_exhaustive]
pub enum BlobError {
    #[error("encountered a dangling Blob ID: `{id}`, the blob store may be corrupt")]
    DanglingBlob { id: BlobId },
    #[error("blob chunk `{id}` failed its content-hash check; refusing to serve tampered data")]
    IntegrityMismatch { id: BlobId },
    #[error("blob `{id}` meta graph is corrupt: {reason}")]
    CorruptGraph { id: BlobId, reason: &'static str },
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
        self.root.join(id.to_string())
    }

    /// Get the path for a blob stored in a package/version directory
    ///
    /// # Errors
    /// Returns an error if `package` or `version` is not a safe path component.
    pub fn application_blob_path(
        &self,
        package: &str,
        version: &str,
        id: BlobId,
    ) -> EyreResult<Utf8PathBuf> {
        utils::validate_path_component(package, Some("package"))?;
        utils::validate_path_component(version, Some("version"))?;

        Ok(self
            .root
            .join("applications")
            .join(package)
            .join(version)
            .join("blobs")
            .join(id.to_string()))
    }

    /// Get the package directory path
    ///
    /// # Errors
    /// Returns an error if `package` is not a safe path component.
    pub fn package_path(&self, package: &str) -> EyreResult<Utf8PathBuf> {
        utils::validate_path_component(package, Some("package"))?;

        Ok(self.root.join("applications").join(package))
    }

    /// Get the version directory path
    ///
    /// # Errors
    /// Returns an error if `package` or `version` is not a safe path component.
    pub fn version_path(&self, package: &str, version: &str) -> EyreResult<Utf8PathBuf> {
        // Validate both components explicitly so the path-traversal contract is
        // self-contained here, rather than leaning on `package_path` to guard
        // `package` indirectly (matches `application_blob_path`).
        utils::validate_path_component(package, Some("package"))?;
        utils::validate_path_component(version, Some("version"))?;

        Ok(self.root.join("applications").join(package).join(version))
    }

    /// Get the root/base path of the blobstore
    pub fn root_path(&self) -> Utf8PathBuf {
        self.root.clone()
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

#[cfg(test)]
mod delete_tests {
    use std::path::Path;
    use std::sync::Arc;

    use calimero_store::db::InMemoryDB;
    use calimero_store::Store as DataStore;
    use camino::Utf8PathBuf;
    use tempfile::tempdir;

    use super::*;

    async fn manager(root: &Path) -> BlobManager {
        let data_store = DataStore::new(Arc::new(InMemoryDB::owned()));
        let config = BlobStoreConfig::new(Utf8PathBuf::from_path_buf(root.to_path_buf()).unwrap());
        let blob_store = FileSystem::new(&config).await.unwrap();
        BlobManager::new(data_store, blob_store)
    }

    #[tokio::test]
    async fn delete_removes_metadata_so_has_stays_consistent() {
        let dir = tempdir().unwrap();
        let mgr = manager(dir.path()).await;

        let data = b"hello blob world";
        let (id, _hash, _size) = mgr.put(&data[..]).await.unwrap();

        assert!(mgr.has(id).unwrap());
        assert!(mgr.get(id).unwrap().is_some());

        assert!(mgr.delete(id).await.unwrap());

        // The metadata row must be gone too; otherwise `has` keeps reporting the
        // blob as present while `get` can no longer read it.
        assert!(!mgr.has(id).unwrap());
        assert!(mgr.get(id).unwrap().is_none());

        // A second delete has nothing left to remove.
        assert!(!mgr.delete(id).await.unwrap());
    }
}

#[cfg(test)]
mod traversal_tests {
    use std::path::Path;
    use std::sync::Arc;

    use calimero_store::db::InMemoryDB;
    use calimero_store::Store as DataStore;
    use camino::Utf8PathBuf;
    use futures_util::TryStreamExt;
    use tempfile::tempdir;

    use super::*;

    async fn manager(root: &Path) -> BlobManager {
        let data_store = DataStore::new(Arc::new(InMemoryDB::owned()));
        let config = BlobStoreConfig::new(Utf8PathBuf::from_path_buf(root.to_path_buf()).unwrap());
        let blob_store = FileSystem::new(&config).await.unwrap();
        BlobManager::new(data_store, blob_store)
    }

    async fn collect(mgr: &BlobManager, id: BlobId) -> Result<Vec<u8>, BlobError> {
        let blob = mgr.get(id).unwrap().expect("blob should exist");
        let chunks: Vec<Box<[u8]>> = blob.try_collect().await?;
        Ok(chunks.concat())
    }

    /// A zero-byte blob has no stored chunk file, but must still read back as
    /// empty rather than surfacing as a `DanglingBlob`.
    #[tokio::test]
    async fn empty_blob_reads_back_empty() {
        let dir = tempdir().unwrap();
        let mgr = manager(dir.path()).await;

        let (id, _hash, size) = mgr.put(&b""[..]).await.unwrap();
        assert_eq!(size, 0);

        let bytes = collect(&mgr, id)
            .await
            .expect("empty blob must be readable");
        assert!(bytes.is_empty(), "empty blob must yield no bytes");
    }

    /// A normal round-trip still works with the content-hash verification in
    /// place (the honest chunk hashes to its own id).
    #[tokio::test]
    async fn roundtrip_verifies_and_serves() {
        let dir = tempdir().unwrap();
        let mgr = manager(dir.path()).await;

        let data = b"the quick brown fox jumps over the lazy dog";
        let (id, _hash, _size) = mgr.put(&data[..]).await.unwrap();

        let bytes = collect(&mgr, id).await.expect("honest blob must serve");
        assert_eq!(bytes, data);
    }

    /// A chunk tampered on disk must fail its content-hash check instead of
    /// being served as authentic.
    #[tokio::test]
    async fn tampered_chunk_is_rejected() {
        let dir = tempdir().unwrap();
        let mgr = manager(dir.path()).await;

        let data = b"authentic payload";
        let (id, _hash, _size) = mgr.put(&data[..]).await.unwrap();

        // The root's single link points at the leaf chunk actually stored on
        // disk; overwrite that file with different bytes.
        let root_meta = mgr
            .data_store
            .handle()
            .get(&BlobMetaKey::new(id))
            .unwrap()
            .unwrap();
        let leaf_id = root_meta.links[0].blob_id();
        mgr.blob_store.put(leaf_id, b"tampered!!").await.unwrap();

        let err = collect(&mgr, id)
            .await
            .expect_err("tampered chunk must be rejected");
        assert!(
            matches!(err, BlobError::IntegrityMismatch { id } if id == leaf_id),
            "expected IntegrityMismatch, got {err:?}"
        );
    }

    /// A back-edge (cycle) in the meta graph must be rejected, not looped over
    /// forever.
    #[tokio::test]
    async fn cycle_in_meta_graph_is_rejected() {
        let dir = tempdir().unwrap();
        let mgr = manager(dir.path()).await;

        let a = BlobId::from([1u8; 32]);
        let b = BlobId::from([2u8; 32]);
        let mut handle = mgr.data_store.handle();
        // A -> B and B -> A: both are internal nodes (non-empty links).
        handle
            .put(
                &BlobMetaKey::new(a),
                &BlobMetaValue::new(1, *a, vec![BlobMetaKey::new(b)].into_boxed_slice()),
            )
            .unwrap();
        handle
            .put(
                &BlobMetaKey::new(b),
                &BlobMetaValue::new(1, *b, vec![BlobMetaKey::new(a)].into_boxed_slice()),
            )
            .unwrap();

        let err = collect(&mgr, a)
            .await
            .expect_err("a cyclic meta graph must be rejected");
        assert!(
            matches!(err, BlobError::CorruptGraph { .. }),
            "expected CorruptGraph, got {err:?}"
        );
    }
}
