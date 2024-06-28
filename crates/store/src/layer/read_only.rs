use crate::iter::{Iter, Structured};
use crate::key::{AsKeyParts, FromKeyParts};
use crate::layer::{Layer, ReadLayer};
use crate::slice::Slice;

pub struct ReadOnly<'k, L> {
    inner: &'k L,
}

impl<'k, L> ReadOnly<'k, L>
where
    L: ReadLayer<'k>,
{
    pub fn new(layer: &'k L) -> Self {
        Self { inner: layer }
    }
}

impl<'k, L: Layer> Layer for ReadOnly<'k, L> {
    type Base = L;
}

impl<'k, L> ReadLayer<'k> for ReadOnly<'k, L>
where
    L: ReadLayer<'k>,
{
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
