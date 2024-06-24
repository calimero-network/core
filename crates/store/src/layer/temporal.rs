use crate::iter::{Iter, IterPair, Structured};
use crate::key::{AsKeyParts, FromKeyParts};
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
        match self.shadow.get(key) {
            Some(Operation::Delete) => Ok(false),
            Some(Operation::Put { .. }) => Ok(true),
            None => self.inner.has(key),
        }
    }

    fn get(&self, key: &'key impl AsKeyParts) -> eyre::Result<Option<Slice>> {
        match self.shadow.get(key) {
            Some(Operation::Delete) => Ok(None),
            Some(Operation::Put { value }) => Ok(Some(value.into())),
            None => self.inner.get(key),
        }
    }

    fn iter<K: AsKeyParts + FromKeyParts>(
        &self,
        start: &'key K,
    ) -> eyre::Result<Iter<Structured<K>>> {
        let inner = self.inner.iter(start)?;
        let shadow = self.shadow.iter_range(start);

        Ok(Iter::new(IterPair(inner, shadow)))
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

// todo! impl calimero_runtime_primitives::Storage for Temporal
// todo!      to get rid of the TemporalRuntimeStore in node
