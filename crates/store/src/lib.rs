use std::sync::Arc;

use eyre::Result as EyreResult;

#[cfg(feature = "datatypes")]
pub mod batch;
pub mod config;
pub mod db;
pub mod entry;
pub mod handle;
pub mod iter;
pub mod key;
pub mod layer;
pub mod slice;
pub mod tx;

use config::StoreConfig;
use db::{Column, Database};
pub use handle::Handle;
use slice::Slice;

#[cfg(feature = "datatypes")]
pub mod types;

#[cfg(feature = "datatypes")]
pub use batch::StoreBatch;

#[derive(Clone, Debug)]
pub struct Store {
    db: Arc<dyn for<'a> Database<'a>>,
}

impl Store {
    /// Creates a new `Store` from an existing database instance. Useful for testing.
    pub fn new(db: Arc<dyn for<'a> Database<'a>>) -> Self {
        Self { db }
    }

    pub fn open<T: for<'a> Database<'a>>(config: &StoreConfig) -> EyreResult<Self> {
        let db = T::open(config)?;
        Ok(Self { db: Arc::new(db) })
    }

    #[must_use]
    pub fn handle(&self) -> Handle<Self> {
        Handle::new(self.clone())
    }

    /// Best-effort on-disk byte estimate for `col` over `[start, end)`.
    /// Backed by `Database::approximate_size` — for RocksDB this is sampled
    /// from SST metadata (sub-millisecond, no scan); in-memory / default
    /// backends fall back to summing `key+value` lengths.
    pub fn approximate_size(
        &self,
        col: Column,
        start: &[u8],
        end: &[u8],
    ) -> EyreResult<u64> {
        self.db.approximate_size(col, Slice::from(start), Slice::from(end))
    }
}
