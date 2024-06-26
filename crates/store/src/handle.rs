use crate::layer::Layer;
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
