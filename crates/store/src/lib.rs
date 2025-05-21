use std::sync::Arc;

use eyre::Result as EyreResult;

pub mod config;
pub mod db;
pub mod entry;
mod handle;
pub mod iter;
pub mod key;
pub mod layer;
pub mod slice;
pub mod tx;

use config::StoreConfig;
use db::Database;
use handle::Handle;

#[cfg(feature = "datatypes")]
pub mod types;

#[derive(Clone, Debug)]
pub struct Store {
    db: Arc<dyn for<'a> Database<'a>>,
}

impl Store {
    pub fn open<T: for<'a> Database<'a>>(config: &StoreConfig) -> EyreResult<Self> {
        let db = T::open(config)?;
        Ok(Self { db: Arc::new(db) })
    }

    #[must_use]
    pub fn handle(&self) -> Handle<Self> {
        Handle::new(self.clone())
    }
}
