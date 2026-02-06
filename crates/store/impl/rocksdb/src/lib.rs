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
//! - Block cache: 128MB LRU cache for frequently accessed blocks
//!
//! If you need to adjust these settings, modify the `Options` in `RocksDB::open()`.

#[cfg(test)]
mod tests;

use calimero_store::config::StoreConfig;
use calimero_store::db::{Column, Database};
use calimero_store::iter::{DBIter, Iter};
use calimero_store::slice::Slice;
use calimero_store::tx::{Operation, Transaction};
use eyre::{bail, Result as EyreResult};
use rocksdb::{
    ColumnFamily, DBRawIteratorWithThreadMode, Options, ReadOptions, Snapshot, WriteBatch, DB,
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

impl RocksDB {
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
        let mut options = Options::default();

        options.create_if_missing(true);
        options.create_missing_column_families(true);

        // Limit file descriptor usage. RocksDB manages an internal LRU cache
        // for file handles, automatically closing least-recently-used files
        // when this limit is reached.
        options.set_max_open_files(DEFAULT_MAX_OPEN_FILES);

        // Configure block cache for better read performance.
        // This cache stores frequently accessed data blocks in memory.
        let cache = rocksdb::Cache::new_lru_cache(DEFAULT_BLOCK_CACHE_SIZE);
        let mut block_opts = rocksdb::BlockBasedOptions::default();
        block_opts.set_block_cache(&cache);
        options.set_block_based_table_factory(&block_opts);

        Ok(Self {
            db: DB::open_cf(&options, &config.path, Column::iter())?,
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
