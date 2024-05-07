use crate::key::KeyParts;
use crate::layer::{ReadLayer, WriteLayer};
use crate::slice::Slice;
use crate::tx::{Operation, Transaction};

pub struct TemporalStore<L> {
    inner: L,
    shadow: Transaction,
}

impl<L: WriteLayer> TemporalStore<L> {
    pub fn new(layer: L) -> Self {
        Self {
            inner: layer,
            shadow: Transaction::default(),
        }
    }
}

impl<L: WriteLayer> ReadLayer for TemporalStore<L> {
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

impl<L: WriteLayer> WriteLayer for TemporalStore<L> {
    type Base = L;

    fn put(&mut self, key: impl KeyParts, value: Slice) -> eyre::Result<()> {
        self.shadow.put(
            key,
            value
                .try_into()
                .unwrap_or_else(|value: Slice| value.as_ref().into()),
        );

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
