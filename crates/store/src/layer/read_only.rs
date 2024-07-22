use crate::iter::{Iter, Structured};
use crate::key::{AsKeyParts, FromKeyParts};
use crate::layer::{Layer, ReadLayer};
use crate::slice::Slice;

pub struct ReadOnly<'base, L> {
    inner: &'base L,
}

impl<'base, 'r, L> ReadOnly<'base, L>
where
    L: ReadLayer<'r>,
{
    pub fn new(layer: &'base L) -> Self {
        Self { inner: layer }
    }
}

impl<'base, L: Layer> Layer for ReadOnly<'base, L> {
    type Base = L;
}

impl<'base, 'r, L> ReadLayer<'r> for ReadOnly<'r, L>
where
    L: ReadLayer<'r>,
{
    fn has(&self, key: &'r impl AsKeyParts) -> eyre::Result<bool> {
        self.inner.has(key)
    }

    fn get(&self, key: &'r impl AsKeyParts) -> eyre::Result<Option<Slice>> {
        self.inner.get(key)
    }

    fn iter<K: AsKeyParts + FromKeyParts>(
        &self,
        start: &'r K,
    ) -> eyre::Result<Iter<Structured<K>>> {
        self.inner.iter(start)
    }
}
