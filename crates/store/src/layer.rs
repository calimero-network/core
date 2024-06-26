use crate::iter::{Iter, Structured};
use crate::key::{AsKeyParts, FromKeyParts};
use crate::slice::Slice;
use crate::tx::Transaction;

// todo!
// mod cache;
mod experiments;
pub mod read_only;
pub mod temporal;

pub trait Layer {
    type Base: Layer;
}

pub trait ReadLayer<'k>: Layer {
    fn has(&self, key: &'k impl AsKeyParts) -> eyre::Result<bool>;
    fn get(&self, key: &'k impl AsKeyParts) -> eyre::Result<Option<Slice>>;
    fn iter<K: AsKeyParts + FromKeyParts>(&self, start: &'k K)
        -> eyre::Result<Iter<Structured<K>>>;
}

pub trait WriteLayer<'k, 'v>: ReadLayer<'k> {
    fn put(&mut self, key: &'k impl AsKeyParts, value: Slice<'v>) -> eyre::Result<()>;
    fn delete(&mut self, key: &'k impl AsKeyParts) -> eyre::Result<()>;
    fn apply(&mut self, tx: &Transaction<'k, 'v>) -> eyre::Result<()>;

    fn commit(self) -> eyre::Result<()>;
}

impl Layer for crate::Store {
    type Base = Self;
}

impl<'k> ReadLayer<'k> for crate::Store {
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

impl<'k, 'v> WriteLayer<'k, 'v> for crate::Store {
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
