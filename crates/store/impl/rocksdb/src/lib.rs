#[cfg(test)]
mod tests;

use calimero_store::config::StoreConfig;
use calimero_store::db::{Column, Database};
use calimero_store::iter::{DBIter, Iter};
use calimero_store::slice::Slice;
use calimero_store::tx::{Operation, Transaction};
use eyre::{bail, Result as EyreResult};
use rocksdb::{ColumnFamily, DBRawIteratorWithThreadMode, Options, ReadOptions, Snapshot, WriteBatch, DB};
use strum::IntoEnumIterator;

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

        // Configure block cache for better performance
        // Default: 128MB LRU cache for frequently accessed blocks
        const DEFAULT_BLOCK_CACHE_SIZE: usize = 128 * 1024 * 1024; // 128MB

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
