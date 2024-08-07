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

pub trait ReadLayer: Layer {
    fn has<K: AsKeyParts>(&self, key: &K) -> eyre::Result<bool>;
    fn get<K: AsKeyParts>(&self, key: &K) -> eyre::Result<Option<Slice>>;
    fn iter<K: FromKeyParts>(&self) -> eyre::Result<Iter<Structured<K>>>;
}

pub trait WriteLayer<'a>: Layer {
    fn put<K: AsKeyParts>(&mut self, key: &'a K, value: Slice<'a>) -> eyre::Result<()>;
    fn delete<K: AsKeyParts>(&mut self, key: &'a K) -> eyre::Result<()>;
    fn apply(&mut self, tx: &Transaction<'a>) -> eyre::Result<()>;

    fn commit(self) -> eyre::Result<()>;
}

pub trait LayerExt: Layer + Sized {
    fn handle(self) -> Handle<Self>;

    fn temporal<'a>(&mut self) -> temporal::Temporal<'_, 'a, Self>
    where
        Self: WriteLayer<'a>,
    {
        temporal::Temporal::new(self)
    }

    fn read_only(&self) -> read_only::ReadOnly<'_, Self>
    where
        Self: ReadLayer,
    {
        read_only::ReadOnly::new(self)
    }
}

impl<L: Layer> LayerExt for L {
    fn handle(self) -> Handle<Self> {
        Handle::new(self)
    }
}

impl Layer for Store {
    type Base = Self;
}

impl ReadLayer for Store {
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

impl<'a> WriteLayer<'a> for Store {
    fn put<K: AsKeyParts>(&mut self, key: &'a K, value: Slice<'a>) -> eyre::Result<()> {
        self.db.put(K::column(), key.as_key().as_slice(), value)
    }

    fn delete<K: AsKeyParts>(&mut self, key: &K) -> eyre::Result<()> {
        self.db.delete(K::column(), key.as_key().as_slice())
    }

    fn apply(&mut self, tx: &Transaction<'a>) -> eyre::Result<()> {
        self.db.apply(tx)
    }

    fn commit(self) -> eyre::Result<()> {
        Ok(())
    }
}
