use crate::layer::{read_only, temporal, Layer, ReadLayer, WriteLayer};
use crate::Store;

mod layer;

pub struct StoreHandle<L = Store> {
    pub(crate) inner: L,
}

impl<L: Layer> StoreHandle<L> {
    pub fn new(inner: L) -> Self {
        Self { inner }
    }

    // todo! can remove when/if Layer::commit() -> Layer::Base
    pub fn into_inner(self) -> L {
        self.inner
    }
}

impl<'k, L: ReadLayer<'k>> StoreHandle<L> {
    pub fn read_only(&'k self) -> read_only::ReadOnly<'k, Self> {
        read_only::ReadOnly::new(self)
    }
}

impl<'base, 'k, 'v, L: WriteLayer<'k, 'v>> StoreHandle<L> {
    pub fn temporal(&'base mut self) -> temporal::Temporal<'base, 'k, 'v, Self> {
        temporal::Temporal::new(self)
    }
}
