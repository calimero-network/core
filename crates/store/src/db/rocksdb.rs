use crate::config::StoreConfig;
use crate::db::{Database, Key, Value};

pub struct RocksDB {
    inner: rocksdb::DB,
    /* options: rocksdb::Options, */
}

impl RocksDB {
    pub fn open(config: &StoreConfig) -> eyre::Result<Self> {
        let mut options = rocksdb::Options::default();
        options.create_if_missing(true);

        let inner = rocksdb::DB::open(&options, &config.path)?;

        Ok(Self {
            inner, /* , options */
        })
    }
}

impl Database for RocksDB {
    fn get(&self, key: &Key) -> eyre::Result<Option<Vec<u8>>> {
        match self.inner.get(key)? {
            Some(value) => Ok(Some(value.to_vec())),
            None => Ok(None),
        }
    }

    fn put(&self, key: &Key, value: Value) -> eyre::Result<()> {
        self.inner.put(key, value)?;

        Ok(())
    }

    fn apply(&self, tx: super::Transaction) -> eyre::Result<()> {
        let mut batch = rocksdb::WriteBatch::default();

        for op in tx.ops {
            match op {
                super::Operation::Put { key, value } => batch.put(key, value),
                super::Operation::Delete { key } => batch.delete(key),
            }
        }

        self.inner.write(batch)?;

        Ok(())
    }
}
