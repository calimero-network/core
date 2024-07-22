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

    fn iter(&self, col: Column, key: Slice) -> eyre::Result<Iter> {
        let cf_handle = self.try_cf_handle(&col)?;

        Ok(Iter::new(DBIterator {
            iter: self.db.raw_iterator_cf(cf_handle),
            seek: Some(key.into_boxed().into()),
        }))
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

pub struct DBIterator<'a, 'k> {
    seek: Option<Slice<'k>>,
    iter: rocksdb::DBRawIterator<'a>,
}

impl<'a, 'k> DBIter for DBIterator<'a, 'k> {
    fn next(&mut self) -> eyre::Result<Option<Slice>> {
        if let Some(seek) = self.seek.take() {
            self.iter.seek(seek);
        } else {
            self.iter.next();
        }

        Ok(self.iter.key().map(Into::into))
    }

    fn read(&self) -> Option<Slice> {
        self.iter.value().map(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use tempdir::TempDir;

    use super::RocksDB;
    use crate::config::StoreConfig;
    use crate::db::{Column, Database};
    use crate::slice::Slice;

    #[test]
    fn test_rocksdb() {
        let dir = TempDir::new("_calimero_store_rocks").unwrap();

        let config = StoreConfig {
            path: dir.path().to_owned().try_into().unwrap(),
        };

        let db = RocksDB::open(&config).unwrap();

        for b1 in 0..10 {
            for b2 in 0..10 {
                let bytes = [b1, b2];

                let key = Slice::from(&bytes[..]);
                let value = Slice::from(&bytes[..]);

                db.put(Column::Identity, (&key).into(), (&value).into())
                    .unwrap();

                assert!(db.has(Column::Identity, (&key).into()).unwrap());
                assert_eq!(db.get(Column::Identity, key).unwrap().unwrap(), value);
            }
        }

        assert_eq!(None, db.get(Column::Identity, (&[]).into()).unwrap());

        let mut iter = db.iter(Column::Identity, (&[]).into()).unwrap();

        let mut entries = iter.entries();

        for b1 in 0..10 {
            for b2 in 0..10 {
                let bytes = [b1, b2];

                let key = Slice::from(&bytes[..]);
                let value = Slice::from(&bytes[..]);

                let (k, v) = entries.next().unwrap();

                assert_eq!(k, key);
                assert_eq!(v, value);
            }
        }
    }
}
