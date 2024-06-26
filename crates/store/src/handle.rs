use crate::layer::{read_only, temporal, Layer, ReadLayer, WriteLayer};
use crate::Store;

pub struct StoreHandle<L = Store> {
    pub(crate) inner: L,
}

impl<L: Layer> StoreHandle<L> {
    pub fn new(inner: L) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> L {
        self.inner
    }
}

impl<'k, L: ReadLayer<'k>> StoreHandle<L> {
    pub fn read_only(&'k self) -> read_only::ReadOnly<'k, L> {
        read_only::ReadOnly::new(&self.inner)
    }
}

impl<'base, 'k, 'v, L: WriteLayer<'k, 'v>> StoreHandle<L> {
    pub fn temporal(&'base mut self) -> temporal::Temporal<'base, 'k, 'v, L> {
        temporal::Temporal::new(&mut self.inner)
    }
}
