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
/// parts), so these caps sit far above any legitimate graph and only trip on
/// corrupt or malicious meta.
///
/// `MAX_BLOB_DEPTH` bounds the chain of *internal* nodes (a second guard against
/// cycles); `MAX_BLOB_NODES` caps the number of *distinct internal* nodes
/// walked. Leaves are neither depth- nor count-limited here — they don't recurse
/// and duplicates are legitimate — so a large file's many chunk leaves are fine.
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
    fn hint(&self) -> usize {
        // The size is a u64; on a 32-bit host a plain cast would truncate and
        // mis-size the links vector. Clamp to usize::MAX instead — it only ever
        // feeds `Vec::with_capacity`, so a saturated hint is harmless.
        match self {
            Self::Hint(size) | Self::Exact(size) => usize::try_from(*size).unwrap_or(usize::MAX),
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

    /// Release one reference to `id` and physically remove it once the last
    /// reference is gone.
    ///
    /// Blobs are content-addressed and deduplicated, so the same bytes may be
    /// shared by several owners (added more than once) or, as a chunk, by
    /// several root blobs. Deleting eagerly would let one owner destroy content
    /// another still references, so a blob's file and metadata are dropped only
    /// once its reference count falls to zero. For a root blob this releases one
    /// reference to each of its chunks too — symmetric with [`Self::put_sized`],
    /// which increments the root and every chunk on add.
    ///
    /// Returns `true` if `id` existed (its reference was released), `false` if
    /// it was already absent. Note that `true` does *not* imply the bytes were
    /// physically removed — if other owners still reference the content, only
    /// the count was decremented and the blob remains readable for them.
    ///
    /// The read-decrement-write is not atomic against a concurrent add/delete of
    /// the *same* content id; callers today drive blob lifecycle serially per
    /// id (see the INVARIANT on [`Self::release_ref`]). Making it fully atomic
    /// would move the refcount update onto the store's transaction path
    /// ([`calimero_store::Store::apply`]).
    pub async fn delete(&self, id: BlobId) -> EyreResult<bool> {
        let Some(meta) = self.data_store.handle().get(&BlobMetaKey::new(id))? else {
            // No metadata row references this id, so as a *referenced blob* it was
            // already absent — return `false` regardless of whether an orphan file
            // happened to linger. Still sweep any such file best-effort (ignoring
            // its outcome) so `has` (metadata-only) and `get` (file-backed) stay
            // consistent.
            let _ = self.blob_store.delete(id).await;
            return Ok(false);
        };

        // Release the root's own reference. A root blob keeps its content in its
        // chunks and has no backing file of its own, so `blob_store.delete` on
        // the root id is a harmless no-op; it is kept for blobs that ever carry
        // their own file. A failed file delete is logged, not propagated, so we
        // still go on to release the chunk references below.
        if self.release_ref(id, &meta)? {
            if let Err(err) = self.blob_store.delete(id).await {
                tracing::warn!(%id, %err, "failed to delete root blob file during delete");
            }
        }

        // Release one reference from every chunk, mirroring the per-chunk
        // increment on add. A chunk shared by another still-live root keeps a
        // positive count and survives. This loop is best-effort: a failure on one
        // chunk is logged and skipped rather than propagated, so it cannot leave
        // the *remaining* chunks with their reference counts un-decremented
        // (which would leak them permanently).
        for link in &meta.links {
            let chunk_id = link.blob_id();
            let chunk_meta = match self.data_store.handle().get(&BlobMetaKey::new(chunk_id)) {
                Ok(Some(chunk_meta)) => chunk_meta,
                Ok(None) => continue,
                Err(err) => {
                    tracing::warn!(%chunk_id, %err, "failed to read chunk metadata during delete; skipping");
                    continue;
                }
            };
            match self.release_ref(chunk_id, &chunk_meta) {
                Ok(true) => {
                    if let Err(err) = self.blob_store.delete(chunk_id).await {
                        tracing::warn!(%chunk_id, %err, "failed to delete chunk file during delete");
                    }
                }
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(%chunk_id, %err, "failed to release chunk reference during delete");
                }
            }
        }

        Ok(true)
    }

    /// Decrement `id`'s reference count by one. Returns `true` when the count
    /// reaches zero — the metadata row is deleted and the caller must remove the
    /// backing file — or `false` when references remain, in which case the
    /// decremented count is persisted and the blob is kept.
    ///
    /// INVARIANT: this and [`Self::persist_ref`] read-modify-write a single
    /// metadata row across separate store operations and are NOT atomic. Callers
    /// must serialize add/delete for the same blob id (the node drives blob
    /// lifecycle serially per id today). A concurrent add interleaved with a
    /// delete could lose an increment and free content that still has a live
    /// owner; making it atomic means moving the update onto the store's
    /// transaction path ([`calimero_store::Store::apply`]).
    fn release_ref(&self, id: BlobId, meta: &BlobMetaValue) -> EyreResult<bool> {
        let key = BlobMetaKey::new(id);
        // refs should never be 0 for a stored entry; if it is (store corruption
        // or a bug that wrote a zero-ref row), surface it rather than silently
        // "fixing" it, then proceed to reclaim the row via the zero arm below.
        if meta.refs == 0 {
            tracing::warn!(%id, "release_ref on a blob with refs == 0; reclaiming as last reference");
        }
        match meta.refs.saturating_sub(1) {
            0 => {
                self.data_store.handle().delete(&key)?;
                Ok(true)
            }
            remaining => {
                self.data_store.handle().put(
                    &key,
                    &BlobMetaValue::new(meta.size, meta.hash, meta.links.clone(), remaining),
                )?;
                Ok(false)
            }
        }
    }

    /// Persist `id`'s metadata on add, incrementing its reference count when an
    /// entry already exists. Deduplicated content re-added by another owner (or
    /// a chunk shared by another root) bumps the count instead of silently
    /// aliasing a single reference that the first delete would tear down.
    ///
    /// INVARIANT: like [`Self::release_ref`], the read-modify-write here is not
    /// atomic; callers must serialize add/delete for the same blob id.
    fn persist_ref(
        &self,
        id: BlobId,
        size: u64,
        hash: [u8; 32],
        links: Box<[BlobMetaKey]>,
    ) -> EyreResult<()> {
        let key = BlobMetaKey::new(id);
        let refs = match self.data_store.handle().get(&key)? {
            // Overflow is not physically reachable (it needs u32::MAX live
            // references to one content id) but is surfaced rather than saturated:
            // a saturated count could never decrement back to zero, permanently
            // leaking the blob.
            Some(existing) => existing.refs.checked_add(1).ok_or_else(|| {
                eyre::eyre!("blob refcount overflow for {id}: already at u32::MAX references")
            })?,
            None => 1,
        };
        self.data_store
            .handle()
            .put(&key, &BlobMetaValue::new(size, hash, links, refs))?;
        Ok(())
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

                self.persist_ref(id, blob.size as u64, *id, Box::default())?;

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

        self.persist_ref(id, size, *hash, links.into_boxed_slice())?;

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

/// Load a leaf blob's bytes and verify them against their content-addressed id.
///
/// A leaf's id IS the sha256 of its bytes, so re-hashing rejects a tampered or
/// corrupt on-disk / peer-supplied chunk (`IntegrityMismatch`) instead of
/// serving it as authentic. A zero-byte blob has no stored file, so it resolves
/// to `None` (nothing to serve) rather than a `DanglingBlob`.
async fn load_verified_leaf(
    blob_mgr: &BlobManager,
    id: BlobId,
    size: u64,
) -> Result<Option<Box<[u8]>>, BlobError> {
    if size == 0 {
        trace!(%id, "empty blob, nothing to serve");
        return Ok(None);
    }

    let bytes = blob_mgr
        .blob_store
        .get(id)
        .await
        .map_err(BlobError::RepoError)?
        .ok_or(BlobError::DanglingBlob { id })?;

    let actual = *AsRef::<[u8; 32]>::as_ref(&Sha256::digest(&bytes));
    if actual != *id {
        error!(%id, "blob chunk hash mismatch; refusing to serve");
        return Err(BlobError::IntegrityMismatch { id });
    }

    Ok(Some(bytes))
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
            // Streaming pre-order DFS over the meta graph. The old version
            // recursed via `Self::new` per link, so a deep chain overflowed the
            // stack and a cycle looped forever. Each frame is a *cursor* over one
            // internal node's links, so at most `depth` link-lists are held at
            // once — the old recursive walk's memory profile, never the whole
            // graph materialised.
            //
            // Cycles: `visited` tracks *internal* nodes only. `put` never shares
            // an internal node, so a revisit is a genuine back-edge — reject it.
            // Duplicate *leaves* are deliberately NOT rejected: repeated file
            // content hashes to the same part id, so a valid blob's links can
            // name the same leaf twice and each occurrence must be served.
            let mut visited: HashSet<BlobId> = HashSet::new();
            let mut chunk_index: u64 = 0;

            // The root may itself be a leaf: a single stored part, or an empty
            // (zero-byte) blob that has no stored file at all.
            if root_meta.links.is_empty() {
                if let Some(bytes) = load_verified_leaf(&blob_mgr, id, root_meta.size).await? {
                    chunk_index += 1;
                    trace!(
                        ?id,
                        chunk_index,
                        chunk_size = bytes.len(),
                        "serving verified blob chunk"
                    );
                    yield bytes;
                }
            } else {
                let _ = visited.insert(id);
                // Frame = (links, next index into them, depth of these children).
                let mut stack: Vec<(Box<[BlobMetaKey]>, usize, usize)> =
                    vec![(root_meta.links, 0, 1)];

                while let Some((links, mut idx, depth)) = stack.pop() {
                    if idx >= links.len() {
                        continue;
                    }
                    let child_id = links[idx].blob_id();
                    idx += 1;
                    let parent = (links, idx, depth);

                    let child_meta = blob_mgr
                        .data_store
                        .handle()
                        .get(&BlobMetaKey::new(child_id))
                        .map_err(|e| BlobError::RepoError(e.into()))?
                        .ok_or_else(|| {
                            error!(?id, missing_child = %child_id, "blob metadata missing referenced child");
                            BlobError::DanglingBlob { id: child_id }
                        })?;

                    if child_meta.links.is_empty() {
                        // Leaf: resume the parent afterwards, then serve this
                        // chunk (an empty leaf yields nothing).
                        stack.push(parent);
                        if let Some(bytes) =
                            load_verified_leaf(&blob_mgr, child_id, child_meta.size).await?
                        {
                            chunk_index += 1;
                            trace!(?id, %child_id, chunk_index, chunk_size = bytes.len(), "serving verified blob chunk");
                            yield bytes;
                        }
                        continue;
                    }

                    // Internal node: bound the internal chain depth, reject a
                    // true cycle (a revisited internal node), cap the distinct
                    // internal-node count, then descend before the next sibling
                    // (LIFO: push the parent cursor first, the child on top).
                    if depth >= MAX_BLOB_DEPTH {
                        error!(?id, %child_id, depth, "blob meta graph exceeds depth budget");
                        Err(BlobError::CorruptGraph {
                            id,
                            reason: "meta graph exceeds depth budget",
                        })?;
                    }
                    if !visited.insert(child_id) {
                        error!(?id, %child_id, "cycle detected in blob meta graph");
                        Err(BlobError::CorruptGraph {
                            id,
                            reason: "meta graph contains a cycle",
                        })?;
                    }
                    if visited.len() > MAX_BLOB_NODES {
                        error!(
                            ?id,
                            nodes = visited.len(),
                            "blob meta graph exceeds node budget"
                        );
                        Err(BlobError::CorruptGraph {
                            id,
                            reason: "meta graph exceeds node budget",
                        })?;
                    }
                    stack.push(parent);
                    stack.push((child_meta.links, 0, depth + 1));
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

    fn refs_of(mgr: &BlobManager, id: BlobId) -> Option<u32> {
        mgr.data_store
            .handle()
            .get(&BlobMetaKey::new(id))
            .unwrap()
            .map(|meta| meta.refs)
    }

    async fn read_all(mgr: &BlobManager, id: BlobId) -> Vec<u8> {
        let mut stream = mgr.get(id).unwrap().expect("blob present");
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            out.extend_from_slice(&chunk.unwrap());
        }
        out
    }

    #[tokio::test]
    async fn duplicate_content_is_refcounted_not_aliased() {
        let dir = tempdir().unwrap();
        let mgr = manager(dir.path()).await;

        // Two owners add byte-identical content. Content addressing makes them
        // one stored blob; without refcounting the first delete would destroy
        // the bytes the second owner still relies on.
        let data = b"identical bytes shared by two owners";
        let (id, _, _) = mgr.put(&data[..]).await.unwrap();
        let (id2, _, _) = mgr.put(&data[..]).await.unwrap();
        assert_eq!(id, id2, "identical content must dedup to the same id");
        assert_eq!(
            refs_of(&mgr, id),
            Some(2),
            "second add must bump the refcount"
        );

        // First owner releases their reference: the blob survives for the second.
        assert!(mgr.delete(id).await.unwrap());
        assert_eq!(refs_of(&mgr, id), Some(1));
        assert!(mgr.has(id).unwrap());
        assert_eq!(
            read_all(&mgr, id).await,
            data,
            "surviving owner keeps its data"
        );

        // Second owner releases the last reference: now it is really gone.
        assert!(mgr.delete(id).await.unwrap());
        assert_eq!(refs_of(&mgr, id), None);
        assert!(!mgr.has(id).unwrap());
    }

    #[tokio::test]
    async fn chunk_shared_across_roots_survives_sibling_delete() {
        let dir = tempdir().unwrap();
        let mgr = manager(dir.path()).await;

        // Two different files that share an identical leading chunk. Their root
        // blobs differ, but the shared chunk is stored once (same content id).
        let prefix = vec![7_u8; CHUNK_SIZE];
        let mut a = prefix.clone();
        a.extend_from_slice(b"divergent tail A");
        let mut b = prefix.clone();
        b.extend_from_slice(b"divergent tail B");

        let (root_a, _, _) = mgr.put(&a[..]).await.unwrap();
        let (root_b, _, _) = mgr.put(&b[..]).await.unwrap();
        assert_ne!(
            root_a, root_b,
            "different content must yield different roots"
        );

        // The shared leading chunk is the first link of both roots.
        let links_a = mgr
            .data_store
            .handle()
            .get(&BlobMetaKey::new(root_a))
            .unwrap()
            .unwrap()
            .links;
        let links_b = mgr
            .data_store
            .handle()
            .get(&BlobMetaKey::new(root_b))
            .unwrap()
            .unwrap()
            .links;
        let shared_chunk = links_a[0].blob_id();
        assert_eq!(
            shared_chunk,
            links_b[0].blob_id(),
            "roots must share a chunk"
        );
        assert_eq!(
            refs_of(&mgr, shared_chunk),
            Some(2),
            "shared chunk carries two refs"
        );

        let tail_a = links_a[1].blob_id();

        // Delete the first root. Its unique tail chunk goes; the shared chunk is
        // decremented but kept, and the sibling blob reads back intact.
        assert!(mgr.delete(root_a).await.unwrap());
        assert!(!mgr.has(root_a).unwrap());
        assert!(
            !mgr.has(tail_a).unwrap(),
            "unique chunk of deleted root is freed"
        );
        assert_eq!(
            refs_of(&mgr, shared_chunk),
            Some(1),
            "shared chunk survives"
        );
        assert_eq!(
            read_all(&mgr, root_b).await,
            b,
            "sibling blob is not corrupted"
        );

        // Deleting the sibling now frees the shared chunk for good.
        assert!(mgr.delete(root_b).await.unwrap());
        assert_eq!(refs_of(&mgr, shared_chunk), None);
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

    /// A file whose content repeats across chunk boundaries produces duplicate
    /// leaf links (identical bytes hash to the same part id). Those duplicates
    /// are legitimate and every occurrence must be served — the traversal must
    /// NOT mistake a repeated leaf for a cycle.
    #[tokio::test]
    async fn repeated_chunks_round_trip() {
        let dir = tempdir().unwrap();
        let mgr = manager(dir.path()).await;

        // Two identical full chunks -> `links` = [part, part] with one part id.
        let data = vec![0xABu8; CHUNK_SIZE * 2];
        let (id, _hash, size) = mgr.put(&data[..]).await.unwrap();
        assert_eq!(size as usize, data.len());

        // Sanity: the root really does reference the same leaf twice.
        let root_meta = mgr
            .data_store
            .handle()
            .get(&BlobMetaKey::new(id))
            .unwrap()
            .unwrap();
        assert_eq!(root_meta.links.len(), 2);
        assert_eq!(root_meta.links[0].blob_id(), root_meta.links[1].blob_id());

        let bytes = collect(&mgr, id)
            .await
            .expect("repeated-chunk blob must serve both copies");
        assert_eq!(
            bytes, data,
            "both duplicate chunks must be served, in order"
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
