use crate::iter::Iter;
use crate::key::AsKeyParts;
use crate::layer::{Layer, ReadLayer};
use crate::slice::Slice;

pub struct ReadOnly<L> {
    inner: L,
}

impl<'k, L> ReadOnly<L>
where
    L: ReadLayer<'k>,
{
    pub fn new(layer: L) -> Self {
        Self { inner: layer }
    }
}

impl<L: Layer> Layer for ReadOnly<L> {
    type Base = L;
}

impl<'k, L> ReadLayer<'k> for ReadOnly<L>
where
    L: ReadLayer<'k>,
{
    fn has(&self, key: &'k impl AsKeyParts) -> eyre::Result<bool> {
        self.inner.has(key)
    }

    fn get(&self, key: &'k impl AsKeyParts) -> eyre::Result<Option<Slice>> {
        self.inner.get(key)
    }

    fn iter(&self, start: &'k impl AsKeyParts) -> eyre::Result<Iter> {
        self.inner.iter(start)
    }
}
