use eyre::Result as EyreResult;

use crate::iter::{Iter, Structured};
use crate::key::{AsKeyParts, FromKeyParts};
use crate::layer::read_only::ReadOnly;
use crate::layer::temporal::Temporal;
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
    fn has<K: AsKeyParts>(&self, key: &K) -> EyreResult<bool>;
    fn get<K: AsKeyParts>(&self, key: &K) -> EyreResult<Option<Slice<'_>>>;

    // TODO: We should consider returning Iterator here.
    #[expect(
        clippy::iter_not_returning_iterator,
        reason = "TODO: This should be implemented"
    )]
    fn iter<K: FromKeyParts>(&self) -> EyreResult<Iter<'_, Structured<K>>>;
}

pub trait WriteLayer<'a>: Layer {
    fn put<K: AsKeyParts>(&mut self, key: &'a K, value: Slice<'a>) -> EyreResult<()>;
    fn delete<K: AsKeyParts>(&mut self, key: &'a K) -> EyreResult<()>;
    fn apply(&mut self, tx: &Transaction<'a>) -> EyreResult<()>;

    fn commit(&mut self) -> EyreResult<()>;
}

pub trait LayerExt: Layer + Sized {
    fn handle(self) -> Handle<Self>;

    fn temporal<'a>(&mut self) -> Temporal<'_, 'a, Self>
    where
        Self: WriteLayer<'a>,
    {
        Temporal::new(self)
    }

    fn read_only(&self) -> ReadOnly<'_, Self>
    where
        Self: ReadLayer,
    {
        ReadOnly::new(self)
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
    fn has<K: AsKeyParts>(&self, key: &K) -> EyreResult<bool> {
        self.db.has(K::column(), key.as_key().as_slice())
    }

    fn get<K: AsKeyParts>(&self, key: &K) -> EyreResult<Option<Slice<'_>>> {
        self.db.get(K::column(), key.as_key().as_slice())
    }

    fn iter<K: FromKeyParts>(&self) -> EyreResult<Iter<'_, Structured<K>>> {
        Ok(self.db.iter(K::column())?.structured_key())
    }
}

impl<'a> WriteLayer<'a> for Store {
    fn put<K: AsKeyParts>(&mut self, key: &'a K, value: Slice<'a>) -> EyreResult<()> {
        self.db.put(K::column(), key.as_key().as_slice(), value)
    }

    fn delete<K: AsKeyParts>(&mut self, key: &K) -> EyreResult<()> {
        self.db.delete(K::column(), key.as_key().as_slice())
    }

    fn apply(&mut self, tx: &Transaction<'a>) -> EyreResult<()> {
        self.db.apply(tx)
    }

    fn commit(&mut self) -> EyreResult<()> {
        Ok(())
    }
}
