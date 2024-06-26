use crate::iter::{Iter, Structured};
use crate::key::{AsKeyParts, FromKeyParts};
use crate::slice::Slice;
use crate::tx::Transaction;
use crate::StoreHandle;

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

pub trait LayerExt: Sized {
    fn handle(self) -> StoreHandle<Self>;
}

impl<L: Layer> LayerExt for L {
    fn handle(self) -> StoreHandle<Self> {
        StoreHandle::new(self)
    }
}
