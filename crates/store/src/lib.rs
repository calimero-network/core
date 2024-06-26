use std::sync::Arc;

pub mod config;
pub mod db;
mod handle;
pub mod iter;
pub mod key;
pub mod layer;
pub mod slice;
mod tx;

pub use handle::StoreHandle;
use iter::{Iter, Structured};
use key::{AsKeyParts, FromKeyParts};
use layer::{Layer, ReadLayer, WriteLayer};
use slice::Slice;
use tx::Transaction;

#[derive(Clone)]
pub struct Store {
    db: Arc<dyn db::Database>,
}

impl Store {
    pub fn open<T: db::Database>(config: &config::StoreConfig) -> eyre::Result<Self> {
        let db = T::open(&config)?;

        Ok(Self { db: Arc::new(db) })
    }
}

impl Layer for Store {
    type Base = Self;
}

impl<'k> ReadLayer<'k> for Store {
    fn has(&self, key: &impl AsKeyParts) -> eyre::Result<bool> {
        let (col, key) = key.parts();

        self.db.has(col, key.as_slice())
    }

    fn get(&self, key: &impl AsKeyParts) -> eyre::Result<Option<Slice>> {
        let (col, key) = key.parts();

        self.db.get(col, key.as_slice())
    }

    fn iter<K: AsKeyParts + FromKeyParts>(&self, start: &K) -> eyre::Result<Iter<Structured<K>>> {
        let (col, key) = start.parts();

        Ok(self.db.iter(col, key.as_slice())?.structured())
    }
}

impl<'k, 'v> WriteLayer<'k, 'v> for Store {
    fn put(&mut self, key: &'k impl AsKeyParts, value: Slice<'v>) -> eyre::Result<()> {
        let (col, key) = key.parts();

        self.db.put(col, key.as_slice(), value)
    }

    fn delete(&mut self, key: &'k impl AsKeyParts) -> eyre::Result<()> {
        let (col, key) = key.parts();

        self.db.delete(col, key.as_slice())
    }

    fn apply(&mut self, tx: &Transaction<'k, 'v>) -> eyre::Result<()> {
        self.db.apply(tx)
    }

    fn commit(self) -> eyre::Result<()> {
        Ok(())
    }
}
