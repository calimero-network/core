use crate::key::KeyParts;
use crate::layer::{Layer, ReadLayer, WriteLayer};
use crate::slice::Slice;
use crate::tx::{Operation, Transaction};

pub struct Temporal<L: WriteLayer> {
    inner: L,
    shadow: Transaction,
}

impl<L: WriteLayer> Temporal<L> {
    pub fn new(layer: L) -> Self {
        Self {
            inner: layer,
            shadow: Transaction::default(),
        }
    }
}

impl<L: WriteLayer> Layer for Temporal<L> {
    type Base = L;

    /// Unwraps the layer, discarding any changes.
    fn unwrap(self) -> Self::Base {
        self.inner
    }
}

impl<L: WriteLayer> ReadLayer for Temporal<L> {
    fn has(&self, key: impl KeyParts) -> eyre::Result<bool> {
        if self.shadow.get(key).is_some() {
            return Ok(true);
        }

        self.inner.has(key)
    }

    fn get(&self, key: impl KeyParts) -> eyre::Result<Option<Slice>> {
        if let Some(Operation::Put { value }) = self.shadow.get(key) {
            return Ok(Some(value.into()));
        }

        self.inner.get(key)
    }
}

impl<L: WriteLayer> WriteLayer for Temporal<L> {
    fn put(&mut self, key: impl KeyParts, value: Slice) -> eyre::Result<()> {
        self.shadow.put(key, value.into());

        Ok(())
    }

    fn delete(&mut self, key: impl KeyParts) -> eyre::Result<()> {
        self.shadow.delete(key);

        Ok(())
    }

    fn apply(&mut self, tx: Transaction) -> eyre::Result<()> {
        self.shadow.merge(tx);

        Ok(())
    }

    fn commit(self) -> eyre::Result<Self::Base> {
        let mut this = self;

        this.inner.apply(this.shadow)?;

        Ok(this.inner)
    }
}
