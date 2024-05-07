use strum::IntoEnumIterator;

use crate::config::StoreConfig;
use crate::db::{Column, Database};
use crate::slice::Slice;
use crate::tx::{Operation, Transaction};

pub struct RocksDB {
    db: rocksdb::DB,
}

impl RocksDB {
    pub fn open(config: &StoreConfig) -> eyre::Result<Self> {
        let mut options = rocksdb::Options::default();

        options.create_if_missing(true);
        options.create_missing_column_families(true);

        Ok(Self {
            db: rocksdb::DB::open_cf(&options, &config.path, Column::iter())?,
        })
    }

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

impl Database for RocksDB {
    fn has(&self, col: Column, key: Slice) -> eyre::Result<bool> {
        let cf_handle = self.try_cf_handle(&col)?;

        let exists = self.db.key_may_exist_cf(cf_handle, key.as_ref());

        Ok(exists)
    }

    fn get(&self, col: Column, key: Slice) -> eyre::Result<Option<Slice>> {
        let cf_handle = self.try_cf_handle(&col)?;

        let value = self.db.get_pinned_cf(cf_handle, key.as_ref())?;

        Ok(value.map(From::from))
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

    fn apply(&self, tx: Transaction) -> eyre::Result<()> {
        let mut batch = rocksdb::WriteBatch::default();

        let mut unknown_cfs = vec![];

        for (entry, op) in tx {
            let Some(cf) = self.cf_handle(&entry.column) else {
                unknown_cfs.push(entry.column);
                continue;
            };
            match op {
                Operation::Put { value } => batch.put_cf(cf, entry.key, value),
                Operation::Delete => batch.delete_cf(cf, entry.key),
            }
        }

        if !unknown_cfs.is_empty() {
            eyre::bail!("unknown column families: {:?}", unknown_cfs);
        }

        self.db.write(batch)?;

        Ok(())
    }
}
