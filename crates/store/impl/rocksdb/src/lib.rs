#[cfg(test)]
mod tests;

use calimero_store::config::StoreConfig;
use calimero_store::db::{Column, Database};
use calimero_store::iter::{DBIter, Iter};
use calimero_store::slice::Slice;
use calimero_store::tx::{Operation, Transaction};
use eyre::{bail, Result as EyreResult};
use rocksdb::{ColumnFamily, DBRawIterator, Options, WriteBatch, DB};
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
}

struct DBIterator<'a> {
    ready: bool,
    iter: DBRawIterator<'a>,
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
