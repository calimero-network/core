//! A tantivy [`Directory`] kept in the node's store.
//!
//! Every file of one `(context, index)` pair lives in [`Column::SearchIndex`],
//! split into [`CHUNK_SIZE`] chunks keyed
//! `context(32) ‖ len(index) u8 ‖ index ‖ len(file) u16 ‖ file ‖ chunk_no u32 BE`,
//! with the file's length under `chunk_no = u32::MAX`. So:
//!
//! - the index inherits the store's at-rest encryption, and sits in the same
//!   RocksDB WAL as the state it is derived from;
//! - a context's whole index is one range delete over its `context` prefix;
//! - a read touches only the chunks its byte range covers.
//!
//! Segment files are immutable once written, so their chunks are cached in a
//! [`ChunkCache`] shared by every directory. `meta.json` and `.managed.json`
//! are the only files tantivy rewrites, and it does so through
//! [`Directory::atomic_write`], which lands as a single write batch.
//!
//! `sync_directory` is a no-op on purpose. The index is derived data: losing
//! its last commit costs a replay of the dirty log, never correctness, and the
//! dirty log is trimmed only *after* a commit, in the same WAL, so a trim that
//! survived a crash implies the commit it follows survived too.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Write};
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::{fmt, mem};

use calimero_primitives::utils::prefix_upper_bound;
use calimero_store::db::Column;
use calimero_store::slice::Slice;
use calimero_store::tx::Transaction;
use calimero_store::Store;
use tantivy::directory::error::{DeleteError, LockError, OpenReadError, OpenWriteError};
use tantivy::directory::{
    AntiCallToken, DirectoryLock, FileHandle, Lock, OwnedBytes, TerminatingWrite, WatchCallback,
    WatchCallbackList, WatchHandle, WritePtr,
};
use tantivy::{Directory, HasLen};

/// Bytes per stored chunk.
pub const CHUNK_SIZE: usize = 64 * 1024;

/// The `chunk_no` a file's length row sits under.
const LEN_ROW: u32 = u32::MAX;

/// Byte counters for one directory, for the benchmark.
#[derive(Debug, Default)]
pub struct IoStats {
    /// Rows put (chunks and length rows).
    pub rows_written: AtomicU64,
    /// Value bytes put.
    pub bytes_written: AtomicU64,
    /// Chunk reads that went to the store.
    pub chunk_reads: AtomicU64,
    /// Chunk reads the cache answered.
    pub cache_hits: AtomicU64,
}

impl IoStats {
    /// `(rows_written, bytes_written, chunk_reads, cache_hits)`.
    #[must_use]
    pub fn snapshot(&self) -> (u64, u64, u64, u64) {
        (
            self.rows_written.load(Ordering::Relaxed),
            self.bytes_written.load(Ordering::Relaxed),
            self.chunk_reads.load(Ordering::Relaxed),
            self.cache_hits.load(Ordering::Relaxed),
        )
    }
}

/// An LRU of immutable file chunks, bounded in bytes, shared by all
/// directories of a node. Keyed by the chunk's full store key, so two contexts
/// can never read each other's bytes out of it.
///
/// It also holds tantivy's lock files. Those are process-local here: one node
/// process owns its store, so the set of held `(index, lock path)` keys among
/// the directories sharing this cache is the whole of it.
pub struct ChunkCache {
    budget: usize,
    inner: Mutex<CacheInner>,
    locks: Mutex<HashSet<Vec<u8>>>,
}

#[derive(Default)]
struct CacheInner {
    map: HashMap<Vec<u8>, (Arc<[u8]>, u64)>,
    by_tick: BTreeMap<u64, Vec<u8>>,
    tick: u64,
    bytes: usize,
}

impl fmt::Debug for ChunkCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChunkCache")
            .field("budget", &self.budget)
            .field("bytes", &self.bytes())
            .finish()
    }
}

impl ChunkCache {
    /// A cache holding at most `budget` bytes of chunks.
    #[must_use]
    pub fn new(budget: usize) -> Arc<Self> {
        Arc::new(Self {
            budget,
            inner: Mutex::default(),
            locks: Mutex::default(),
        })
    }

    /// Bytes currently cached.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .bytes
    }

    fn get(&self, key: &[u8]) -> Option<Arc<[u8]>> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.tick += 1;
        let tick = inner.tick;
        let (chunk, old) = {
            let (chunk, at) = inner.map.get_mut(key)?;
            (Arc::clone(chunk), mem::replace(at, tick))
        };
        let key = inner.by_tick.remove(&old)?;
        let _ = inner.by_tick.insert(tick, key);
        Some(chunk)
    }

    fn put(&self, key: Vec<u8>, chunk: Arc<[u8]>) {
        if self.budget == 0 || chunk.len() > self.budget {
            return;
        }
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.tick += 1;
        let tick = inner.tick;
        inner.bytes += chunk.len();
        if let Some((old, at)) = inner.map.insert(key.clone(), (chunk, tick)) {
            inner.bytes -= old.len();
            let _ = inner.by_tick.remove(&at);
        }
        let _ = inner.by_tick.insert(tick, key);
        while inner.bytes > self.budget {
            let Some((_, key)) = inner.by_tick.pop_first() else {
                break;
            };
            if let Some((chunk, _)) = inner.map.remove(&key) {
                inner.bytes -= chunk.len();
            }
        }
    }

    fn forget_prefix(&self, prefix: &[u8]) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let doomed: Vec<(Vec<u8>, u64)> = inner
            .map
            .iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .map(|(key, (_, at))| (key.clone(), *at))
            .collect();
        for (key, at) in doomed {
            if let Some((chunk, _)) = inner.map.remove(&key) {
                inner.bytes -= chunk.len();
            }
            let _ = inner.by_tick.remove(&at);
        }
    }
}

/// The key prefix of every row of `context`'s search indexes.
#[must_use]
pub fn context_prefix(context: &[u8; 32]) -> Vec<u8> {
    context.to_vec()
}

/// The key prefix of every file of one index.
#[must_use]
pub fn index_prefix(context: &[u8; 32], index: &str) -> Vec<u8> {
    let name = index.as_bytes();
    let len = u8::try_from(name.len()).unwrap_or(u8::MAX);
    let mut out = Vec::with_capacity(32 + 1 + name.len());
    out.extend_from_slice(context);
    out.push(len);
    out.extend_from_slice(&name[..usize::from(len)]);
    out
}

struct HeldLock {
    table: Arc<ChunkCache>,
    key: Vec<u8>,
}

impl Drop for HeldLock {
    fn drop(&mut self) {
        let _ = self
            .table
            .locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

struct Inner {
    store: Store,
    prefix: Vec<u8>,
    cache: Arc<ChunkCache>,
    watchers: WatchCallbackList,
    stats: IoStats,
}

/// The [`Directory`] of one `(context, index)` pair, over [`Column::SearchIndex`].
#[derive(Clone)]
pub struct RocksDirectory {
    inner: Arc<Inner>,
}

impl fmt::Debug for RocksDirectory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksDirectory")
            .field("prefix", &self.inner.prefix)
            .finish_non_exhaustive()
    }
}

impl RocksDirectory {
    /// The directory of `index` in `context`.
    #[must_use]
    pub fn new(store: Store, context: &[u8; 32], index: &str, cache: Arc<ChunkCache>) -> Self {
        Self {
            inner: Arc::new(Inner {
                store,
                prefix: index_prefix(context, index),
                cache,
                watchers: WatchCallbackList::default(),
                stats: IoStats::default(),
            }),
        }
    }

    /// This directory's I/O counters.
    #[must_use]
    pub fn stats(&self) -> &IoStats {
        &self.inner.stats
    }

    /// Bytes of value stored under this directory (a scan; for measurement).
    ///
    /// # Errors
    /// A store read failure.
    pub fn stored_bytes(&self) -> eyre::Result<(u64, u64)> {
        let lo = self.inner.prefix.clone();
        let hi = prefix_upper_bound(&lo);
        let rows = self
            .inner
            .store
            .raw_scan(Column::SearchIndex, &lo, &hi, None)?;
        let bytes = rows.iter().map(|(k, v)| (k.len() + v.len()) as u64).sum();
        Ok((rows.len() as u64, bytes))
    }

    fn file_prefix(&self, path: &Path) -> Vec<u8> {
        file_prefix(&self.inner.prefix, path)
    }

    fn len_of(&self, file: &[u8]) -> io::Result<Option<usize>> {
        let key = chunk_key(file, LEN_ROW);
        let row = self
            .inner
            .store
            .raw_get(Column::SearchIndex, &key)
            .map_err(io::Error::other)?;
        row.map(|bytes| {
            let bytes: [u8; 8] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| io::Error::other("corrupt search file length row"))?;
            usize::try_from(u64::from_le_bytes(bytes)).map_err(io::Error::other)
        })
        .transpose()
    }

    fn put(&self, key: &[u8], value: &[u8]) -> io::Result<()> {
        let _ = self
            .inner
            .stats
            .rows_written
            .fetch_add(1, Ordering::Relaxed);
        let _ = self
            .inner
            .stats
            .bytes_written
            .fetch_add(value.len() as u64, Ordering::Relaxed);
        self.inner
            .store
            .raw_put(Column::SearchIndex, key, value)
            .map_err(io::Error::other)
    }

    fn open_error(path: &Path, error: io::Error) -> OpenReadError {
        OpenReadError::IoError {
            io_error: Arc::new(error),
            filepath: path.to_path_buf(),
        }
    }
}

fn file_prefix(index_prefix: &[u8], path: &Path) -> Vec<u8> {
    let name = path.to_string_lossy();
    let name = name.as_bytes();
    let len = u16::try_from(name.len()).unwrap_or(u16::MAX);
    let mut out = Vec::with_capacity(index_prefix.len() + 2 + name.len());
    out.extend_from_slice(index_prefix);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&name[..usize::from(len)]);
    out
}

fn chunk_key(file: &[u8], chunk: u32) -> Vec<u8> {
    let mut key = Vec::with_capacity(file.len() + 4);
    key.extend_from_slice(file);
    key.extend_from_slice(&chunk.to_be_bytes());
    key
}

/// One stored file, read chunk by chunk.
struct RocksFile {
    dir: RocksDirectory,
    file: Vec<u8>,
    len: usize,
}

impl fmt::Debug for RocksFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RocksFile").field("len", &self.len).finish()
    }
}

impl HasLen for RocksFile {
    fn len(&self) -> usize {
        self.len
    }
}

impl RocksFile {
    fn chunk(&self, no: u32) -> io::Result<Arc<[u8]>> {
        let key = chunk_key(&self.file, no);
        let inner = &self.dir.inner;
        if let Some(chunk) = inner.cache.get(&key) {
            let _ = inner.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(chunk);
        }
        let _ = inner.stats.chunk_reads.fetch_add(1, Ordering::Relaxed);
        let bytes = inner
            .store
            .raw_get(Column::SearchIndex, &key)
            .map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other("search index chunk missing"))?;
        let chunk: Arc<[u8]> = bytes.into();
        inner.cache.put(key, Arc::clone(&chunk));
        Ok(chunk)
    }
}

impl FileHandle for RocksFile {
    fn read_bytes(&self, range: Range<usize>) -> io::Result<OwnedBytes> {
        if range.end > self.len || range.start > range.end {
            return Err(io::Error::other("read past the end of a search index file"));
        }
        if range.is_empty() {
            return Ok(OwnedBytes::empty());
        }
        let first = range.start / CHUNK_SIZE;
        let last = (range.end - 1) / CHUNK_SIZE;
        let first_no = u32::try_from(first).map_err(io::Error::other)?;
        if first == last {
            // The common case: the whole range inside one chunk, served as a
            // zero-copy view of the cached chunk.
            let start = range.start - first * CHUNK_SIZE;
            let end = range.end - first * CHUNK_SIZE;
            return Ok(OwnedBytes::new(self.chunk(first_no)?).slice(start..end));
        }
        let mut out = Vec::with_capacity(range.len());
        for no in first..=last {
            let chunk = self.chunk(u32::try_from(no).map_err(io::Error::other)?)?;
            let base = no * CHUNK_SIZE;
            let start = range.start.max(base) - base;
            let end = range.end.min(base + chunk.len()) - base;
            out.extend_from_slice(&chunk[start..end]);
        }
        Ok(OwnedBytes::new(out))
    }
}

/// A file being written: full chunks go to the store as they fill; the tail
/// and the length row land on `terminate`, which is when tantivy is done.
struct RocksWriter {
    dir: RocksDirectory,
    file: Vec<u8>,
    buf: Vec<u8>,
    next_chunk: u32,
    written: u64,
}

impl RocksWriter {
    fn drain_full_chunks(&mut self) -> io::Result<()> {
        while self.buf.len() >= CHUNK_SIZE {
            let rest = self.buf.split_off(CHUNK_SIZE);
            let chunk = mem::replace(&mut self.buf, rest);
            self.dir
                .put(&chunk_key(&self.file, self.next_chunk), &chunk)?;
            self.next_chunk += 1;
        }
        Ok(())
    }
}

impl Write for RocksWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        self.written += data.len() as u64;
        self.drain_full_chunks()?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.drain_full_chunks()
    }
}

impl TerminatingWrite for RocksWriter {
    fn terminate_ref(&mut self, _: AntiCallToken) -> io::Result<()> {
        self.drain_full_chunks()?;
        if !self.buf.is_empty() {
            let tail = mem::take(&mut self.buf);
            self.dir
                .put(&chunk_key(&self.file, self.next_chunk), &tail)?;
            self.next_chunk += 1;
        }
        self.dir
            .put(&chunk_key(&self.file, LEN_ROW), &self.written.to_le_bytes())
    }
}

impl Directory for RocksDirectory {
    fn get_file_handle(&self, path: &Path) -> Result<Arc<dyn FileHandle>, OpenReadError> {
        let file = self.file_prefix(path);
        let len = self
            .len_of(&file)
            .map_err(|e| Self::open_error(path, e))?
            .ok_or_else(|| OpenReadError::FileDoesNotExist(path.to_path_buf()))?;
        Ok(Arc::new(RocksFile {
            dir: self.clone(),
            file,
            len,
        }))
    }

    fn delete(&self, path: &Path) -> Result<(), DeleteError> {
        let file = self.file_prefix(path);
        let exists = self
            .len_of(&file)
            .map_err(|e| DeleteError::IoError {
                io_error: Arc::new(e),
                filepath: path.to_path_buf(),
            })?
            .is_some();
        if !exists {
            return Err(DeleteError::FileDoesNotExist(path.to_path_buf()));
        }
        let hi = prefix_upper_bound(&file);
        self.inner
            .store
            .raw_delete_range(Column::SearchIndex, &file, &hi)
            .map_err(|e| DeleteError::IoError {
                io_error: Arc::new(io::Error::other(e)),
                filepath: path.to_path_buf(),
            })?;
        self.inner.cache.forget_prefix(&file);
        Ok(())
    }

    fn exists(&self, path: &Path) -> Result<bool, OpenReadError> {
        let file = self.file_prefix(path);
        Ok(self
            .len_of(&file)
            .map_err(|e| Self::open_error(path, e))?
            .is_some())
    }

    fn open_write(&self, path: &Path) -> Result<WritePtr, OpenWriteError> {
        let file = self.file_prefix(path);
        let io_err = |e: io::Error| OpenWriteError::IoError {
            io_error: Arc::new(e),
            filepath: path.to_path_buf(),
        };
        if self.len_of(&file).map_err(io_err)?.is_some() {
            return Err(OpenWriteError::FileAlreadyExists(path.to_path_buf()));
        }
        // Visible (empty) from here on, as tantivy expects of a file it opened.
        self.put(&chunk_key(&file, LEN_ROW), &0_u64.to_le_bytes())
            .map_err(io_err)?;
        Ok(io::BufWriter::with_capacity(
            CHUNK_SIZE,
            Box::new(RocksWriter {
                dir: self.clone(),
                file,
                buf: Vec::new(),
                next_chunk: 0,
                written: 0,
            }),
        ))
    }

    fn atomic_read(&self, path: &Path) -> Result<Vec<u8>, OpenReadError> {
        let handle = self.get_file_handle(path)?;
        let bytes = handle
            .read_bytes(0..handle.len())
            .map_err(|e| Self::open_error(path, e))?;
        Ok(bytes.as_slice().to_vec())
    }

    fn atomic_write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        let file = self.file_prefix(path);
        let old_chunks = self
            .len_of(&file)?
            .map_or(0, |len| len.div_ceil(CHUNK_SIZE));
        let new_chunks = data.len().div_ceil(CHUNK_SIZE);

        let mut tx = Transaction::default();
        for (no, chunk) in data.chunks(CHUNK_SIZE).enumerate() {
            let no = u32::try_from(no).map_err(io::Error::other)?;
            tx.raw_put(
                Column::SearchIndex,
                Slice::from(chunk_key(&file, no)),
                Slice::from(chunk.to_vec()),
            );
        }
        for no in new_chunks..old_chunks {
            let no = u32::try_from(no).map_err(io::Error::other)?;
            tx.raw_delete(Column::SearchIndex, Slice::from(chunk_key(&file, no)));
        }
        tx.raw_put(
            Column::SearchIndex,
            Slice::from(chunk_key(&file, LEN_ROW)),
            Slice::from((data.len() as u64).to_le_bytes().to_vec()),
        );
        let _ = self
            .inner
            .stats
            .rows_written
            .fetch_add(new_chunks as u64 + 1, Ordering::Relaxed);
        let _ = self
            .inner
            .stats
            .bytes_written
            .fetch_add(data.len() as u64 + 8, Ordering::Relaxed);
        self.inner.store.apply(&tx).map_err(io::Error::other)?;
        // A rewritten file must never be served from a stale cached chunk.
        self.inner.cache.forget_prefix(&file);

        if path == Path::new("meta.json") {
            drop(self.inner.watchers.broadcast());
        }
        Ok(())
    }

    fn sync_directory(&self) -> io::Result<()> {
        Ok(())
    }

    fn acquire_lock(&self, lock: &Lock) -> Result<DirectoryLock, LockError> {
        let key = file_prefix(&self.inner.prefix, &lock.filepath);
        let table = Arc::clone(&self.inner.cache);
        if !table
            .locks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone())
        {
            return Err(LockError::LockBusy);
        }
        Ok(DirectoryLock::from(Box::new(HeldLock { table, key })))
    }

    fn watch(&self, watch_callback: WatchCallback) -> tantivy::Result<WatchHandle> {
        Ok(self.inner.watchers.subscribe(watch_callback))
    }
}

/// Remove every search row of `context`: its index files and its dirty log.
///
/// # Errors
/// A store failure.
pub fn delete_context(store: &Store, cache: &ChunkCache, context: &[u8; 32]) -> eyre::Result<()> {
    let lo = context_prefix(context);
    let hi = prefix_upper_bound(&lo);
    store.raw_delete_range(Column::SearchIndex, &lo, &hi)?;
    store.raw_delete_range(Column::SearchDirty, &lo, &hi)?;
    cache.forget_prefix(&lo);
    Ok(())
}

#[cfg(test)]
mod tests {
    use calimero_store::db::InMemoryDB;

    use super::*;

    fn dir(context: u8) -> RocksDirectory {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        RocksDirectory::new(store, &[context; 32], "messages", ChunkCache::new(1 << 20))
    }

    #[test]
    fn a_written_file_reads_back_across_chunk_boundaries() {
        let dir = dir(1);
        let data: Vec<u8> = (0..(CHUNK_SIZE * 3 + 17))
            .map(|i| (i % 251) as u8)
            .collect();
        let mut w = dir.open_write(Path::new("seg.idx")).unwrap();
        w.write_all(&data).unwrap();
        w.terminate().unwrap();

        let handle = dir.get_file_handle(Path::new("seg.idx")).unwrap();
        assert_eq!(handle.len(), data.len());
        for range in [
            0..10,
            CHUNK_SIZE - 3..CHUNK_SIZE + 5,
            5..CHUNK_SIZE * 3 + 10,
            data.len() - 1..data.len(),
        ] {
            assert_eq!(
                handle.read_bytes(range.clone()).unwrap().as_slice(),
                &data[range]
            );
        }
        assert!(handle.read_bytes(0..data.len() + 1).is_err());
    }

    #[test]
    fn open_write_refuses_an_existing_file_and_delete_removes_it() {
        let dir = dir(2);
        let w = dir.open_write(Path::new("a")).unwrap();
        drop(w);
        assert!(dir.exists(Path::new("a")).unwrap());
        assert!(matches!(
            dir.open_write(Path::new("a")),
            Err(OpenWriteError::FileAlreadyExists(_))
        ));
        dir.delete(Path::new("a")).unwrap();
        assert!(!dir.exists(Path::new("a")).unwrap());
        assert!(matches!(
            dir.delete(Path::new("a")),
            Err(DeleteError::FileDoesNotExist(_))
        ));
    }

    #[test]
    fn atomic_write_shrinks_a_file_without_leaving_stale_chunks() {
        let dir = dir(3);
        let big = vec![7_u8; CHUNK_SIZE * 2 + 1];
        dir.atomic_write(Path::new("meta.json"), &big).unwrap();
        assert_eq!(dir.atomic_read(Path::new("meta.json")).unwrap(), big);
        dir.atomic_write(Path::new("meta.json"), b"{}").unwrap();
        assert_eq!(dir.atomic_read(Path::new("meta.json")).unwrap(), b"{}");
        let (rows, _) = dir.stored_bytes().unwrap();
        assert_eq!(rows, 2, "one chunk and one length row");
    }

    #[test]
    fn two_contexts_never_see_each_others_files() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let cache = ChunkCache::new(1 << 20);
        let a = RocksDirectory::new(store.clone(), &[1; 32], "m", Arc::clone(&cache));
        let b = RocksDirectory::new(store.clone(), &[2; 32], "m", Arc::clone(&cache));
        a.atomic_write(Path::new("meta.json"), b"a").unwrap();
        assert!(!b.exists(Path::new("meta.json")).unwrap());
        delete_context(&store, &cache, &[1; 32]).unwrap();
        assert!(!a.exists(Path::new("meta.json")).unwrap());
    }

    #[test]
    fn the_cache_stays_within_its_budget() {
        let cache = ChunkCache::new(100);
        for i in 0..10_u8 {
            cache.put(vec![i], Arc::from(vec![0_u8; 30]));
        }
        assert!(cache.bytes() <= 100);
        assert!(cache.get(&[9]).is_some(), "the newest survives");
        assert!(cache.get(&[0]).is_none(), "the oldest is evicted");
    }
}
