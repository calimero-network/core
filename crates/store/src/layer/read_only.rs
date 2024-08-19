use crate::iter::{Iter, Structured};
use crate::key::{AsKeyParts, FromKeyParts};
use crate::layer::{Layer, ReadLayer};
use crate::slice::Slice;

pub struct ReadOnly<'base, L> {
    inner: &'base L,
}

impl<'base, L> ReadOnly<'base, L>
where
    L: ReadLayer,
{
    pub fn new(layer: &'base L) -> Self {
        Self { inner: layer }
    }
}

impl<'base, L: Layer> Layer for ReadOnly<'base, L> {
    type Base = L;
}

impl<'base, L> ReadLayer for ReadOnly<'base, L>
where
    L: ReadLayer,
{
    fn has<K: AsKeyParts>(&self, key: &K) -> eyre::Result<bool> {
        self.inner.has(key)
    }

    fn get<K: AsKeyParts>(&self, key: &K) -> eyre::Result<Option<Slice>> {
        self.inner.get(key)
    }

    fn iter<K: FromKeyParts>(&self) -> eyre::Result<Iter<Structured<K>>> {
        self.inner.iter()
    }
}
