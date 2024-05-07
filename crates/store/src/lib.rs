use std::sync::Arc;

pub mod config;
mod db;
mod key;
pub mod layer;
pub mod slice;
mod tx;

#[derive(Clone)]
pub struct Store {
    db: Arc<dyn db::Database>,
}

impl Store {
    pub fn open(config: &config::StoreConfig) -> eyre::Result<Self> {
        let db = db::rocksdb::RocksDB::open(&config)?;

        Ok(Self { db: Arc::new(db) })
    }
}
