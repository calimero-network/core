use crate::key::AsKeyParts;
use crate::layer::{Layer, ReadLayer, WriteLayer};
use crate::slice::Slice;
use crate::tx::{Operation, Transaction};

pub struct Temporal<'base, 'key, 'value, L> {
    inner: &'base mut L,
    shadow: Transaction<'key, 'value>,
}

impl<'base, 'key, 'value, L> Temporal<'base, 'key, 'value, L>
where
    L: WriteLayer<'key, 'value>,
{
    pub fn new(layer: &'base mut L) -> Self {
        Self {
            inner: layer,
            shadow: Transaction::default(),
        }
    }
}

impl<'base, 'key, 'value, L> Layer for Temporal<'base, 'key, 'value, L>
where
    L: WriteLayer<'key, 'value>,
{
    type Base = L;
}

impl<'base, 'key, 'value, L> ReadLayer<'key> for Temporal<'base, 'key, 'value, L>
where
    L: WriteLayer<'key, 'value>,
{
    fn has(&self, key: &'key impl AsKeyParts) -> eyre::Result<bool> {
        if self.shadow.get(key).is_some() {
            return Ok(true);
        }

        self.inner.has(key)
    }

    fn get(&self, key: &'key impl AsKeyParts) -> eyre::Result<Option<Slice>> {
        if let Some(Operation::Put { value }) = self.shadow.get(key) {
            return Ok(Some(value.into()));
        }

        self.inner.get(key)
    }
}

impl<'base, 'key, 'value, L> WriteLayer<'key, 'value> for Temporal<'base, 'key, 'value, L>
where
    L: WriteLayer<'key, 'value>,
{
    fn put(&mut self, key: &'key impl AsKeyParts, value: Slice<'value>) -> eyre::Result<()> {
        self.shadow.put(key, value);

        Ok(())
    }

    fn delete(&mut self, key: &'key impl AsKeyParts) -> eyre::Result<()> {
        self.shadow.delete(key);

        Ok(())
    }

    fn apply(&mut self, tx: &Transaction<'key, 'value>) -> eyre::Result<()> {
        self.shadow.merge(tx);

        Ok(())
    }

    fn commit(self) -> eyre::Result<()> {
        self.inner.apply(&self.shadow)?;

        Ok(())
    }
}
