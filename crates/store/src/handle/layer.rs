use crate::handle::StoreHandle;
use crate::iter::{Iter, Structured};
use crate::key::{AsKeyParts, FromKeyParts};
use crate::layer::{Layer, ReadLayer, WriteLayer};
use crate::slice::Slice;
use crate::tx::Transaction;

impl Layer for StoreHandle {
    type Base = Self;
}

impl<'k> ReadLayer<'k> for StoreHandle {
    fn has(&self, key: &impl AsKeyParts) -> eyre::Result<bool> {
        let (col, key) = key.parts();

        self.inner.db.has(col, key.as_slice())
    }

    fn get(&self, key: &impl AsKeyParts) -> eyre::Result<Option<Slice>> {
        let (col, key) = key.parts();

        self.inner.db.get(col, key.as_slice())
    }

    fn iter<K: AsKeyParts + FromKeyParts>(&self, start: &K) -> eyre::Result<Iter<Structured<K>>> {
        let (col, key) = start.parts();

        Ok(self.inner.db.iter(col, key.as_slice())?.structured())
    }
}

impl<'k, 'v> WriteLayer<'k, 'v> for StoreHandle {
    fn put(&mut self, key: &'k impl AsKeyParts, value: Slice<'v>) -> eyre::Result<()> {
        let (col, key) = key.parts();

        self.inner.db.put(col, key.as_slice(), value)
    }

    fn delete(&mut self, key: &'k impl AsKeyParts) -> eyre::Result<()> {
        let (col, key) = key.parts();

        self.inner.db.delete(col, key.as_slice())
    }

    fn apply(&mut self, tx: &Transaction<'k, 'v>) -> eyre::Result<()> {
        self.inner.db.apply(tx)
    }

    fn commit(self) -> eyre::Result<()> {
        Ok(())
    }
}

impl<L: Layer> Layer for StoreHandle<L> {
    type Base = L::Base;
}

impl<'k, L: ReadLayer<'k>> ReadLayer<'k> for StoreHandle<L> {
    fn has(&self, key: &'k impl AsKeyParts) -> eyre::Result<bool> {
        self.inner.has(key)
    }

    fn get(&self, key: &'k impl AsKeyParts) -> eyre::Result<Option<Slice>> {
        self.inner.get(key)
    }

    fn iter<K: AsKeyParts + FromKeyParts>(
        &self,
        start: &'k K,
    ) -> eyre::Result<Iter<Structured<K>>> {
        self.inner.iter(start)
    }
}

impl<'k, 'v, L: WriteLayer<'k, 'v>> WriteLayer<'k, 'v> for StoreHandle<L> {
    fn put(&mut self, key: &'k impl AsKeyParts, value: Slice<'v>) -> eyre::Result<()> {
        self.inner.put(key, value)
    }

    fn delete(&mut self, key: &'k impl AsKeyParts) -> eyre::Result<()> {
        self.inner.delete(key)
    }

    fn apply(&mut self, tx: &Transaction<'k, 'v>) -> eyre::Result<()> {
        self.inner.apply(tx)
    }

    fn commit(self) -> eyre::Result<()> {
        self.inner.commit()
    }
}
