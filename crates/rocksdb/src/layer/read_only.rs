use eyre::Result as EyreResult;

use crate::iter::{Iter, Structured};
use crate::key::{AsKeyParts, FromKeyParts};
use crate::layer::{Layer, ReadLayer};
use crate::slice::Slice;

#[derive(Debug)]
pub struct ReadOnly<'base, L> {
    inner: &'base L,
}

impl<'base, L> ReadOnly<'base, L>
where
    L: ReadLayer,
{
    pub const fn new(layer: &'base L) -> Self {
        Self { inner: layer }
    }
}

impl<L: Layer> Layer for ReadOnly<'_, L> {
    type Base = L;
}

impl<L> ReadLayer for ReadOnly<'_, L>
where
    L: ReadLayer,
{
    fn has<K: AsKeyParts>(&self, key: &K) -> EyreResult<bool> {
        self.inner.has(key)
    }

    fn get<K: AsKeyParts>(&self, key: &K) -> EyreResult<Option<Slice<'_>>> {
        self.inner.get(key)
    }

    fn iter<K: FromKeyParts>(&self) -> EyreResult<Iter<'_, Structured<K>>> {
        self.inner.iter()
    }
}
