use std::sync::Arc;

pub mod config;
pub mod db;
pub mod entry;
mod handle;
pub mod iter;
pub mod key;
pub mod layer;
pub mod slice;
mod tx;
pub mod types;

use handle::StoreHandle;

#[derive(Clone)]
pub struct Store {
    db: Arc<dyn db::Database>,
}

impl Store {
    pub fn open<T: db::Database>(config: &config::StoreConfig) -> eyre::Result<Self> {
        let db = T::open(config)?;

        Ok(Self { db: Arc::new(db) })
    }

    pub fn handle(&self) -> StoreHandle {
        StoreHandle::new(self.clone())
    }
}
