use crate::iter::{Iter, Structured};
use crate::key::{AsKeyParts, FromKeyParts};
use crate::slice::Slice;
use crate::tx::Transaction;
use crate::{Handle, Store};

// todo!
// mod cache;
mod experiments;
pub mod read_only;
pub mod temporal;

pub trait Layer {
    type Base: Layer;
}

pub trait ReadLayer<'r>: Layer {
    fn has<K: AsKeyParts>(&'r self, key: &'r K) -> eyre::Result<bool>;
    fn get<K: AsKeyParts>(&'r self, key: &'r K) -> eyre::Result<Option<Slice<'r>>>;
    fn iter<K: FromKeyParts>(&'r self) -> eyre::Result<Iter<Structured<K>>>;
}

pub trait WriteLayer<'w>: Layer {
    fn put<K: AsKeyParts>(&mut self, key: &'w K, value: Slice<'w>) -> eyre::Result<()>;
    fn delete<K: AsKeyParts>(&mut self, key: &'w K) -> eyre::Result<()>;
    fn apply(&mut self, tx: &Transaction<'w>) -> eyre::Result<()>;

    fn commit(self) -> eyre::Result<()>;
}

pub trait LayerExt: Sized {
    fn handle(self) -> Handle<Self>;

    fn temporal<'entry>(&mut self) -> temporal::Temporal<'_, 'entry, Self>
    where
        Self: WriteLayer<'entry>,
    {
        temporal::Temporal::new(self)
    }

    fn read_only<'a>(&self) -> read_only::ReadOnly<'_, Self>
    where
        Self: ReadLayer<'a>,
    {
        read_only::ReadOnly::new(self)
    }
}

impl<L: Layer> LayerExt for L {
    fn handle(self) -> Handle<Self> {
        Handle::new(self)
    }
}

impl Layer for Store<'_, '_> {
    type Base = Self;
}

impl<'db, 'a> ReadLayer<'a> for Store<'db, 'a> {
    fn has<K: AsKeyParts>(&self, key: &K) -> eyre::Result<bool> {
        self.db.has(K::column(), key.as_key().as_slice())
    }

    fn get<K: AsKeyParts>(&self, key: &K) -> eyre::Result<Option<Slice>> {
        self.db.get(K::column(), key.as_key().as_slice())
    }

    fn iter<K: FromKeyParts>(&self) -> eyre::Result<Iter<Structured<K>>> {
        Ok(self.db.iter(K::column())?.structured_key())
    }
}

impl<'db, 'a> WriteLayer<'a> for Store<'db, 'a> {
    fn put<K: AsKeyParts>(&mut self, key: &'a K, value: Slice<'a>) -> eyre::Result<()> {
        self.db.put(K::column(), key.as_key().as_slice(), value)
    }

    fn delete<K: AsKeyParts>(&mut self, key: &'a K) -> eyre::Result<()> {
        self.db.delete(K::column(), key.as_key().as_slice())
    }

    fn apply(&mut self, tx: &Transaction<'a>) -> eyre::Result<()> {
        self.db.apply(tx)
    }

    fn commit(self) -> eyre::Result<()> {
        Ok(())
    }
}
