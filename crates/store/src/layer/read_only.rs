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

impl<L: Layer> Layer for ReadOnly<'_, L> {
    type Base = L;
}

impl<L> ReadLayer for ReadOnly<'_, L>
where
    L: ReadLayer,
{
    fn has<K: AsKeyParts>(&self, key: &K) -> eyre::Result<bool> {
        self.inner.has(key)
    }

    fn get<K: AsKeyParts>(&self, key: &K) -> eyre::Result<Option<Slice<'_>>> {
        self.inner.get(key)
    }

    fn iter<K: FromKeyParts>(&self) -> eyre::Result<Iter<'_, Structured<K>>> {
        self.inner.iter()
    }
}
