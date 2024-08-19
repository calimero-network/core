#[cfg(test)]
#[path = "../tests/db/rocksdb.rs"]
mod tests;

use strum::IntoEnumIterator;

use crate::config::StoreConfig;
use crate::db::{Column, Database};
use crate::iter::{DBIter, Iter};
use crate::slice::Slice;
use crate::tx::{Operation, Transaction};

pub struct RocksDB {
    db: rocksdb::DB,
}

impl RocksDB {
    fn cf_handle(&self, column: &Column) -> Option<&rocksdb::ColumnFamily> {
        self.db.cf_handle(column.as_ref())
    }

    fn try_cf_handle(&self, column: &Column) -> eyre::Result<&rocksdb::ColumnFamily> {
        let Some(cf_handle) = self.cf_handle(column) else {
            eyre::bail!("unknown column family: {:?}", column);
        };

        Ok(cf_handle)
    }
}

impl Database<'_> for RocksDB {
    fn open(config: &StoreConfig) -> eyre::Result<Self> {
        let mut options = rocksdb::Options::default();

        options.create_if_missing(true);
        options.create_missing_column_families(true);

        Ok(Self {
            db: rocksdb::DB::open_cf(&options, &config.path, Column::iter())?,
        })
    }

    fn has(&self, col: Column, key: Slice) -> eyre::Result<bool> {
        let cf_handle = self.try_cf_handle(&col)?;

        let exists = self.db.key_may_exist_cf(cf_handle, key.as_ref())
            && self.get(col, key).map(|value| value.is_some())?;

        Ok(exists)
    }

    fn get(&self, col: Column, key: Slice) -> eyre::Result<Option<Slice>> {
        let cf_handle = self.try_cf_handle(&col)?;

        let value = self.db.get_pinned_cf(cf_handle, key.as_ref())?;

        Ok(value.map(Slice::from_owned))
    }

    fn put(&self, col: Column, key: Slice, value: Slice) -> eyre::Result<()> {
        let cf_handle = self.try_cf_handle(&col)?;

        self.db.put_cf(cf_handle, key.as_ref(), value.as_ref())?;

        Ok(())
    }

    fn delete(&self, col: Column, key: Slice) -> eyre::Result<()> {
        let cf_handle = self.try_cf_handle(&col)?;

        self.db.delete_cf(cf_handle, key.as_ref())?;

        Ok(())
    }

    fn iter(&self, col: Column) -> eyre::Result<Iter> {
        let cf_handle = self.try_cf_handle(&col)?;

        let mut iter = self.db.raw_iterator_cf(cf_handle);

        iter.seek_to_first();

        Ok(Iter::new(DBIterator { ready: true, iter }))
    }

    fn apply(&self, tx: &Transaction) -> eyre::Result<()> {
        let mut batch = rocksdb::WriteBatch::default();

        let mut unknown_cfs = vec![];

        for (entry, op) in tx.iter() {
            let (col, key) = (entry.column(), entry.key());

            let Some(cf) = self.cf_handle(&col) else {
                unknown_cfs.push(col);
                continue;
            };
            match op {
                Operation::Put { value } => batch.put_cf(cf, key, value),
                Operation::Delete => batch.delete_cf(cf, key),
            }
        }

        if !unknown_cfs.is_empty() {
            eyre::bail!("unknown column families: {:?}", unknown_cfs);
        }

        self.db.write(batch)?;

        Ok(())
    }
}

struct DBIterator<'a> {
    ready: bool,
    iter: rocksdb::DBRawIterator<'a>,
}

impl<'a> DBIter for DBIterator<'a> {
    fn seek(&mut self, key: Slice) -> eyre::Result<Option<Slice>> {
        self.iter.seek(key);

        self.ready = false;

        Ok(self.iter.key().map(Into::into))
    }

    fn next(&mut self) -> eyre::Result<Option<Slice>> {
        if self.ready {
            self.ready = false;
        } else {
            self.iter.next();
        }

        Ok(self.iter.key().map(Into::into))
    }

    fn read(&self) -> eyre::Result<Slice> {
        let Some(value) = self.iter.value() else {
            eyre::bail!("missing value for iterator entry {:?}", self.iter.key());
        };

        Ok(value.into())
    }
}
