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
#[cfg(feature = "datatypes")]
pub mod types;

use handle::Handle;

#[derive(Clone)]
pub struct Store {
    db: Arc<dyn for<'a> db::Database<'a>>,
}

impl Store {
    pub fn open<T: for<'a> db::Database<'a>>(config: &config::StoreConfig) -> eyre::Result<Self> {
        let db = T::open(&config)?;

        Ok(Store { db: Arc::new(db) })
    }

    pub fn handle(&self) -> Handle<Self> {
        Handle::new(self.clone())
    }
}
