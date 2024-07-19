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

pub struct Store<'db, 'a> {
    db: Arc<dyn db::Database<'a> + 'db>,
}

impl Clone for Store<'_, '_> {
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
        }
    }
}

impl<'db, 'a> Store<'db, 'a> {
    pub fn open<T: db::Database<'a> + 'db>(config: &config::StoreConfig) -> eyre::Result<Self> {
        let db = T::open(&config)?;

        Ok(Store { db: Arc::new(db) })
    }

    pub fn handle(&self) -> Handle<Self> {
        Handle::new(self.clone())
    }
}
