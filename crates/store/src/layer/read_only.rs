use crate::key::KeyParts;
use crate::layer::ReadLayer;
use crate::slice::Slice;

pub struct ReadOnly<L> {
    inner: L,
}

impl<L: ReadLayer> ReadOnly<L> {
    pub fn new(layer: L) -> Self {
        Self { inner: layer }
    }
}

impl<L: ReadLayer> ReadLayer for ReadOnly<L> {
    fn has(&self, key: impl KeyParts) -> eyre::Result<bool> {
        self.inner.has(key)
    }

    fn get(&self, key: impl KeyParts) -> eyre::Result<Option<Slice>> {
        self.inner.get(key)
    }
}
