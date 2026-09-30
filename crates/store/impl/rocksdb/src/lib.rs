//! # RocksDB Storage Backend
//!
//! This module provides a RocksDB-based implementation of the `Database` trait.
//!
//! ## Why No Connection Pool?
//!
//! RocksDB already manages resources internally and does not require an external
//! connection pool. Here's why:
//!
//! 1. **Thread Safety**: The `DB` object is thread-safe and designed to be shared
//!    across multiple threads. A single `DB` instance can handle concurrent
//!    read/write operations safely.
//!
//! 2. **Internal File Handle Management**: RocksDB uses an LRU cache to manage
//!    open file handles internally. The `max_open_files` option controls how many
//!    SST files RocksDB keeps open simultaneously. When this limit is reached,
//!    RocksDB automatically closes least-recently-used file handles.
//!
//! 3. **Block Cache**: RocksDB maintains its own block cache for frequently
//!    accessed data, reducing disk I/O without external caching.
//!
//! 4. **File Locking**: RocksDB uses file locking to prevent multiple processes
//!    from opening the same database. Only one `DB` instance can exist per path
//!    per process.
//!
//! 5. **The `Store` Wrapper**: The higher-level `Store` type already wraps the
//!    database in an `Arc`, allowing it to be cloned and shared efficiently
//!    across the application.
//!
//! ## Recommended Usage
//!
//! ```ignore
//! // Open the database once
//! let store = Store::open::<RocksDB>(&config)?;
//!
//! // Clone and share the store (cheap Arc clone)
//! let store_clone = store.clone();
//!
//! // Both handles share the same underlying RocksDB instance
//! ```
//!
//! ## Configuration
//!
//! Resource limits are configured through RocksDB's native options in the `open`
//! method:
//! - `max_open_files`: Controls file descriptor usage (default: 256)
//! - Block cache: one 128MB LRU cache shared by every column family
//! - `max_total_wal_size`: Bounds write-ahead log retention (default: 256MB)
//! - `db_write_buffer_size`: Bounds memtable memory across all families (256MB)
//!
//! Every column family is opened with the options from `column_options`: LZ4
//! compression, ZSTD with a trained dictionary on the bottommost level, bloom
//! filters, and compaction of files dense with deletions. RocksDB applies
//! per-family options only to families opened with their own descriptor, so a
//! setting placed on the DB-wide `Options` alone never reaches the data.
//!
//! If you need to adjust these settings, modify `column_options` or the DB-wide
//! `Options` in `RocksDB::open()`.

#[cfg(test)]
mod tests;

use calimero_store::config::StoreConfig;
use calimero_store::db::{Column, Database};
use calimero_store::iter::{DBIter, Iter};
use calimero_store::slice::Slice;
use calimero_store::tx::{Operation, Transaction};
use eyre::{bail, Result as EyreResult};
use rocksdb::{
    BlockBasedOptions, Cache, ColumnFamily, ColumnFamilyDescriptor, DBCompressionType,
    DBRawIteratorWithThreadMode, Options, ReadOptions, ReadTier, Snapshot, WriteBatch, DB,
};
use strum::IntoEnumIterator;

/// Default maximum number of open files for RocksDB.
///
/// This limits file descriptor usage. RocksDB uses an internal LRU cache
/// to manage file handles when this limit is reached.
const DEFAULT_MAX_OPEN_FILES: i32 = 256;

/// Default block cache size in bytes (128MB).
///
/// The block cache stores frequently accessed data blocks in memory,
/// reducing disk I/O for hot data.
const DEFAULT_BLOCK_CACHE_SIZE: usize = 128 * 1024 * 1024;

/// Cap on the total size of write-ahead logs kept on disk (256MB).
///
/// RocksDB retains every WAL back to the oldest UNFLUSHED memtable across all
/// column families, so a family that is written to rarely pins every log
/// written since it last flushed — however much that is. With the per-family
/// `write_buffer_size` at its 64MB default and 20+ families, cold ones such as
/// `Meta` or `Generic` can go days without reaching that threshold while `State`
/// flushes thousands of times, so the WAL grows without bound.
///
/// Observed on a real node before this cap: 19GB of WAL guarding 109MB of SST
/// data, growing ~10GB/day, pinned by two families that had last flushed two
/// days earlier. Setting this makes RocksDB flush whichever family holds the
/// oldest log once the total crosses the cap, which lets the old logs be
/// deleted. 256MB is far above normal steady-state need here and still bounds
/// the directory.
const DEFAULT_MAX_TOTAL_WAL_SIZE: u64 = 256 * 1024 * 1024;

/// Cap on the memory held by memtables across every column family (256MB).
///
/// Each family otherwise gets its own 64MB write buffer, up to two of them, so
/// 20+ families could in principle hold several gigabytes before any flushed.
/// Past this budget RocksDB flushes the largest memtable instead.
const DEFAULT_DB_WRITE_BUFFER_SIZE: usize = 256 * 1024 * 1024;

/// Bloom filter density: ~1% false positives for ~1.25 bytes per key.
///
/// Without a filter, a point lookup for an absent key reads a data block from
/// every level that might hold it, and `has()` can only short-circuit on keys
/// still in a memtable or the block cache.
const BLOOM_FILTER_BITS_PER_KEY: f64 = 10.0;

/// ZSTD level for the bottommost level, where ~90% of the data settles.
///
/// Level 3 is ZSTD's own default: most of the ratio of the higher levels at a
/// fraction of their compaction CPU.
const BOTTOMMOST_ZSTD_LEVEL: i32 = 3;

/// ZSTD window size for the bottommost level. `-14` is RocksDB's default.
const BOTTOMMOST_ZSTD_WINDOW_BITS: i32 = -14;

/// Size of the ZSTD dictionary trained per bottommost SST (16KB).
///
/// State rows are small and share structure (metadata layout, repeated ids),
/// which block-level compression alone cannot exploit across blocks; a shared
/// dictionary can.
const BOTTOMMOST_ZSTD_DICT_BYTES: i32 = 16 * 1024;

/// Sample size the dictionary is trained on: RocksDB's recommended 100x the
/// dictionary size.
const BOTTOMMOST_ZSTD_TRAIN_BYTES: i32 = BOTTOMMOST_ZSTD_DICT_BYTES * 100;

/// Sliding window, in entries, that the deletion-triggered compaction collector
/// inspects.
const DELETION_COMPACTION_WINDOW: usize = 128 * 1024;

/// Deletions within [`DELETION_COMPACTION_WINDOW`] entries that mark an SST for
/// compaction.
const DELETION_COMPACTION_TRIGGER: usize = 16 * 1024;

/// Share of tombstones in an SST that marks it for compaction.
const DELETION_COMPACTION_RATIO: f64 = 0.5;

/// RocksDB database wrapper implementing the `Database` trait.
///
/// This is a thin wrapper around RocksDB's `DB` type. The `DB` instance is
/// thread-safe and handles its own resource management internally.
///
/// ## Resource Management
///
/// RocksDB manages resources internally - there is no need for an external
/// connection pool. Key points:
///
/// - **Single instance per path**: RocksDB uses file locking; only one `DB`
///   can be open per database path per process.
/// - **Thread-safe**: Share the `RocksDB` instance (or the `Store` wrapper)
///   across threads freely.
/// - **Automatic file handle management**: RocksDB's internal LRU cache
///   manages open file handles based on `max_open_files`.
///
/// ## Sharing
///
/// To share a database across your application, use the `Store` wrapper which
/// provides `Arc`-based sharing, or wrap `RocksDB` in an `Arc` yourself.
#[derive(Debug)]
pub struct RocksDB {
    db: DB,
}

/// Options every column family is opened with.
///
/// Compression is LZ4 on every level but the bottommost, which uses ZSTD with a
/// trained dictionary: with dynamic level sizing (RocksDB's default) the
/// bottommost level holds most of the data, so that is where the stronger
/// codec pays for its CPU, while LZ4 keeps flushes and upper-level compactions
/// cheap.
///
/// Deletion-heavy files are compacted early so point deletes — tombstone GC,
/// delta pruning, trie rows dropped on delete — release their space without
/// waiting for the file to be picked up by size-triggered compaction.
fn column_options(table: &BlockBasedOptions) -> Options {
    let mut options = Options::default();

    options.set_block_based_table_factory(table);

    options.set_compression_type(DBCompressionType::Lz4);
    options.set_bottommost_compression_type(DBCompressionType::Zstd);
    options.set_bottommost_compression_options(
        BOTTOMMOST_ZSTD_WINDOW_BITS,
        BOTTOMMOST_ZSTD_LEVEL,
        0,
        BOTTOMMOST_ZSTD_DICT_BYTES,
        true,
    );
    options.set_bottommost_zstd_max_train_bytes(BOTTOMMOST_ZSTD_TRAIN_BYTES, true);

    options.add_compact_on_deletion_collector_factory(
        DELETION_COMPACTION_WINDOW,
        DELETION_COMPACTION_TRIGGER,
        DELETION_COMPACTION_RATIO,
    );

    options
}

/// Table options shared by every column family: one block cache for the whole
/// database, and a bloom filter per SST.
///
/// Index and filter blocks are charged to the block cache so their memory is
/// bounded by it rather than growing with the data, and L0's are pinned so the
/// files every read checks first never miss.
fn table_options(cache: &Cache) -> BlockBasedOptions {
    let mut table = BlockBasedOptions::default();

    table.set_block_cache(cache);
    table.set_bloom_filter(BLOOM_FILTER_BITS_PER_KEY, false);
    table.set_cache_index_and_filter_blocks(true);
    table.set_pin_l0_filter_and_index_blocks_in_cache(true);

    table
}

impl RocksDB {
    /// Flushes the WAL, then the memtable of every column family.
    ///
    /// WAL first, so that if the second step is interrupted the durable WAL
    /// still covers every write.
    ///
    /// Every column family, not `self.db.flush()`: that maps to the C API's
    /// `rocksdb_flush`, which flushes the DEFAULT family only — and nothing in
    /// this store writes to `default`.
    fn flush_all(&self) -> EyreResult<()> {
        self.db.flush_wal(true)?;

        let handles = Column::iter()
            .map(|column| self.try_cf_handle(column))
            .collect::<EyreResult<Vec<_>>>()?;
        self.db
            .flush_cfs_opt(&handles, &rocksdb::FlushOptions::default())?;

        Ok(())
    }

    fn cf_handle(&self, column: Column) -> Option<&ColumnFamily> {
        self.db.cf_handle(column.as_ref())
    }

    fn try_cf_handle(&self, column: Column) -> EyreResult<&ColumnFamily> {
        let Some(cf_handle) = self.cf_handle(column) else {
            bail!("unknown column family: {:?}", column);
        };

        Ok(cf_handle)
    }
}

impl Database<'_> for RocksDB {
    fn open(config: &StoreConfig) -> EyreResult<Self> {
        let cache = Cache::new_lru_cache(DEFAULT_BLOCK_CACHE_SIZE);
        let table = table_options(&cache);

        // The DB-wide options also configure the `default` family, which
        // RocksDB always opens; give it the same column options as the rest.
        let mut options = column_options(&table);

        options.create_if_missing(true);
        options.create_missing_column_families(true);

        // Limit file descriptor usage. RocksDB manages an internal LRU cache
        // for file handles, automatically closing least-recently-used files
        // when this limit is reached.
        options.set_max_open_files(DEFAULT_MAX_OPEN_FILES);

        // Bound the write-ahead log. Without this RocksDB keeps every WAL back
        // to the oldest unflushed memtable, so one rarely-written column family
        // pins them all (see DEFAULT_MAX_TOTAL_WAL_SIZE).
        options.set_max_total_wal_size(DEFAULT_MAX_TOTAL_WAL_SIZE);

        options.set_db_write_buffer_size(DEFAULT_DB_WRITE_BUFFER_SIZE);

        // One descriptor per family, each with its own copy of the column
        // options. `DB::open_cf` would open every named family with
        // `Options::default()`, leaving all of the above on `default` alone.
        let descriptors = Column::iter()
            .map(|column| ColumnFamilyDescriptor::new(column.as_ref(), column_options(&table)));

        Ok(Self {
            db: DB::open_cf_descriptors(&options, &config.path, descriptors)?,
        })
    }

    fn has(&self, col: Column, key: Slice<'_>) -> EyreResult<bool> {
        let cf_handle = self.try_cf_handle(col)?;

        let exists = self.db.key_may_exist_cf(cf_handle, key.as_ref())
            && self.get(col, key).map(|value| value.is_some())?;

        Ok(exists)
    }

    fn get(&self, col: Column, key: Slice<'_>) -> EyreResult<Option<Slice<'_>>> {
        let cf_handle = self.try_cf_handle(col)?;

        let value = self.db.get_pinned_cf(cf_handle, key.as_ref())?;

        Ok(value.map(Slice::from_owned))
    }

    fn put(&self, col: Column, key: Slice<'_>, value: Slice<'_>) -> EyreResult<()> {
        let cf_handle = self.try_cf_handle(col)?;

        self.db.put_cf(cf_handle, key.as_ref(), value.as_ref())?;

        Ok(())
    }

    fn delete(&self, col: Column, key: Slice<'_>) -> EyreResult<()> {
        let cf_handle = self.try_cf_handle(col)?;

        self.db.delete_cf(cf_handle, key.as_ref())?;

        Ok(())
    }

    fn iter(&self, col: Column) -> EyreResult<Iter<'_>> {
        let cf_handle = self.try_cf_handle(col)?;

        let mut iter = self.db.raw_iterator_cf(cf_handle);

        iter.seek_to_first();

        Ok(Iter::new(DBIterator { ready: true, iter }))
    }

    fn last_in_range(
        &self,
        col: Column,
        lo: Slice<'_>,
        hi: Slice<'_>,
    ) -> EyreResult<Option<(Vec<u8>, Vec<u8>)>> {
        let cf_handle = self.try_cf_handle(col)?;
        let mut iter = self.db.raw_iterator_cf(cf_handle);

        // `seek_for_prev(hi)` lands on the largest key <= hi; `hi` is the
        // exclusive upper bound, so step back past any key == hi to reach the
        // largest key strictly < hi. One seek + a step or two — O(log n).
        iter.seek_for_prev(hi.as_ref());
        while iter.valid() {
            // Copy the key out before any mutation drops the borrow.
            let Some(key) = iter.key().map(<[u8]>::to_vec) else {
                break;
            };
            if key.as_slice() >= hi.as_ref() {
                iter.prev();
                continue;
            }
            if key.as_slice() < lo.as_ref() {
                return Ok(None);
            }
            let value = iter.value().map(<[u8]>::to_vec).unwrap_or_default();
            return Ok(Some((key, value)));
        }
        Ok(None)
    }

    fn delete_range(&self, col: Column, lo: Slice<'_>, hi: Slice<'_>) -> EyreResult<()> {
        let cf_handle = self.try_cf_handle(col)?;
        // A single range tombstone over `[lo, hi)` — no per-key delete, no scan,
        // and no unbounded in-memory key buffer.
        self.db
            .delete_range_cf(cf_handle, lo.as_ref(), hi.as_ref())?;
        Ok(())
    }

    fn compact_range(&self, col: Column, lo: Slice<'_>, hi: Slice<'_>) -> EyreResult<()> {
        let cf_handle = self.try_cf_handle(col)?;
        self.db
            .compact_range_cf(cf_handle, Some(lo.as_ref()), Some(hi.as_ref()));
        Ok(())
    }

    /// Estimated bytes stored in `[start, end)`: flushed SST files plus the
    /// memtables that have not been flushed yet.
    ///
    /// `get_approximate_sizes_cf` samples SST metadata — no scan, typically
    /// sub-millisecond — but RocksDB's C API calls it with the files-only
    /// flag, so anything still buffered in a memtable counts as zero. With a
    /// 64MB write buffer per column family, a small or freshly written range
    /// can live entirely in memory and would report 0 bytes indefinitely.
    ///
    /// The memtable half is a real scan, restricted to the memtable tier, so
    /// its cost is bounded by the write buffer size rather than the range.
    /// A key present in both an SST and a memtable (overwritten since the last
    /// flush) is counted twice, and memtable tombstones are not subtracted:
    /// this is an estimate for quotas and dashboards, not an audit figure.
    fn approximate_size(&self, col: Column, start: Slice<'_>, end: Slice<'_>) -> EyreResult<u64> {
        let cf_handle = self.try_cf_handle(col)?;
        let range = rocksdb::Range::new(start.as_ref(), end.as_ref());
        let sizes = self.db.get_approximate_sizes_cf(cf_handle, &[range]);
        let persisted = sizes.into_iter().next().unwrap_or(0);

        let mut read_opts = ReadOptions::default();
        read_opts.set_read_tier(ReadTier::Memtable);
        // Callers build `end` by incrementing a fixed-length prefix, which
        // wraps to all zeroes when the prefix is all 0xFF. That end sorts
        // before `start`, and means "to the end of the column".
        let bounded = !end.as_ref().iter().all(|byte| *byte == 0);
        if bounded {
            read_opts.set_iterate_upper_bound(end.as_ref().to_vec());
        }
        let mut iter = self.db.raw_iterator_cf_opt(cf_handle, read_opts);
        iter.seek(start.as_ref());
        let mut buffered: u64 = 0;
        while iter.valid() {
            let (Some(key), Some(value)) = (iter.key(), iter.value()) else {
                break;
            };
            buffered = buffered
                .saturating_add(key.len() as u64)
                .saturating_add(value.len() as u64);
            iter.next();
        }
        iter.status()?;

        Ok(persisted.saturating_add(buffered))
    }

    fn apply(&self, tx: &Transaction<'_>) -> EyreResult<()> {
        let mut batch = WriteBatch::default();

        let mut unknown_cfs = vec![];

        for (entry, op) in tx.iter() {
            let (col, key) = (entry.column(), entry.key());

            let Some(cf) = self.cf_handle(col) else {
                unknown_cfs.push(col);
                continue;
            };
            match op {
                Operation::Put { value } => batch.put_cf(cf, key, value),
                Operation::Delete => batch.delete_cf(cf, key),
            }
        }

        if !unknown_cfs.is_empty() {
            bail!("unknown column families: {:?}", unknown_cfs);
        }

        self.db.write(batch)?;

        Ok(())
    }

    fn flush(&self) -> EyreResult<()> {
        // Called on controlled shutdown; `Drop` runs the same sequence for the
        // abrupt path.
        self.flush_all()
    }

    fn iter_snapshot(&self, col: Column) -> EyreResult<Iter<'_>> {
        let cf_handle = self.try_cf_handle(col)?;
        let snapshot = self.db.snapshot();

        // Create read options with the snapshot pinned
        let mut read_opts = ReadOptions::default();
        read_opts.set_snapshot(&snapshot);

        // Create iterator with snapshot-pinned read options
        let mut iter = self.db.raw_iterator_cf_opt(cf_handle, read_opts);
        iter.seek_to_first();

        Ok(Iter::new(SnapshotIterator {
            ready: true,
            iter,
            _snapshot: snapshot,
        }))
    }
}

impl Drop for RocksDB {
    fn drop(&mut self) {
        // Persist the memtable + WAL when the last handle to the database goes
        // away. Without this, an abrupt process stop (kill, container
        // termination) between the last write and RocksDB's background flush
        // loses whatever was still buffered in memory. Errors are swallowed —
        // `drop` cannot propagate and there is nothing to recover to at this
        // point. The controlled-shutdown path calls `flush()` explicitly
        // *before* drop so this is a backstop, not the primary durability
        // barrier. It flushes every column family, like `flush()`: a bare
        // `self.db.flush()` would drain only the unused `default` family.
        let _ = self.flush_all();
    }
}

struct DBIterator<'a> {
    ready: bool,
    iter: DBRawIteratorWithThreadMode<'a, DB>,
}

/// Iterator that holds a RocksDB snapshot for consistent reads.
///
/// The snapshot is stored alongside the iterator to ensure it outlives
/// the iterator. The iterator sees a frozen point-in-time view of the DB.
struct SnapshotIterator<'a> {
    ready: bool,
    /// The raw iterator over the snapshot.
    /// SAFETY: `iter` must be declared before `_snapshot` because Rust drops
    /// struct fields in declaration order (top-to-bottom). The iterator holds
    /// references into the snapshot's data, so it must be dropped first.
    iter: DBRawIteratorWithThreadMode<'a, DB>,
    /// Snapshot must outlive the iterator. Declared after `iter` to ensure
    /// correct drop order.
    _snapshot: Snapshot<'a>,
}

impl DBIter for DBIterator<'_> {
    fn seek(&mut self, key: Slice<'_>) -> EyreResult<Option<Slice<'_>>> {
        self.iter.seek(key);

        self.ready = false;

        Ok(self.iter.key().map(Into::into))
    }

    fn next(&mut self) -> EyreResult<Option<Slice<'_>>> {
        if self.ready {
            self.ready = false;
        } else {
            self.iter.next();
        }

        Ok(self.iter.key().map(Into::into))
    }

    fn read(&self) -> EyreResult<Slice<'_>> {
        let Some(value) = self.iter.value() else {
            bail!("missing value for iterator entry {:?}", self.iter.key());
        };

        Ok(value.into())
    }
}

impl DBIter for SnapshotIterator<'_> {
    fn seek(&mut self, key: Slice<'_>) -> EyreResult<Option<Slice<'_>>> {
        self.iter.seek(key);

        self.ready = false;

        Ok(self.iter.key().map(Into::into))
    }

    fn next(&mut self) -> EyreResult<Option<Slice<'_>>> {
        if self.ready {
            self.ready = false;
        } else {
            self.iter.next();
        }

        Ok(self.iter.key().map(Into::into))
    }

    fn read(&self) -> EyreResult<Slice<'_>> {
        let Some(value) = self.iter.value() else {
            bail!("missing value for iterator entry {:?}", self.iter.key());
        };

        Ok(value.into())
    }
}
