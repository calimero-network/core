use crate::iter::{Iter, IterPair, Structured};
use crate::key::{AsKeyParts, FromKeyParts};
use crate::layer::{Layer, ReadLayer, WriteLayer};
use crate::slice::Slice;
use crate::tx::{Operation, Transaction};

pub struct Temporal<'base, 'entry, L> {
    inner: &'base mut L,
    shadow: Transaction<'entry>,
}

impl<'base, 'entry, L> Temporal<'base, 'entry, L>
where
    L: WriteLayer<'entry>,
{
    pub fn new(layer: &'base mut L) -> Self {
        Self {
            inner: layer,
            shadow: Transaction::default(),
        }
    }
}

impl<'base, 'entry, L> Layer for Temporal<'base, 'entry, L>
where
    L: Layer,
{
    type Base = L;
}

impl<'base, 'entry, L> ReadLayer<'base> for Temporal<'base, 'entry, L>
where
    L: ReadLayer<'base>,
{
    fn has(&'base self, key: &'base impl AsKeyParts) -> eyre::Result<bool> {
        match self.shadow.get(key) {
            Some(Operation::Delete) => Ok(false),
            Some(Operation::Put { .. }) => Ok(true),
            None => self.inner.has(key),
        }
    }

    fn get(&'base self, key: &'base impl AsKeyParts) -> eyre::Result<Option<Slice<'base>>> {
        match self.shadow.get(key) {
            Some(Operation::Delete) => Ok(None),
            Some(Operation::Put { value }) => Ok(Some(value.into())),
            None => self.inner.get(key),
        }
    }

    fn iter<K: AsKeyParts + FromKeyParts>(
        &'base self,
        start: &'base K,
    ) -> eyre::Result<Iter<Structured<K>>> {
        let inner = self.inner.iter(start)?;
        let shadow = self.shadow.iter_range(start);

        Ok(Iter::new(IterPair(inner, shadow)))
    }
}

impl<'base, 'entry, L> WriteLayer<'entry> for Temporal<'base, 'entry, L>
where
    L: WriteLayer<'entry>,
{
    fn put(&mut self, key: &'entry impl AsKeyParts, value: Slice<'entry>) -> eyre::Result<()> {
        self.shadow.put(key, value);

        Ok(())
    }

    fn delete(&mut self, key: &'entry impl AsKeyParts) -> eyre::Result<()> {
        self.shadow.delete(key);

        Ok(())
    }

    fn apply(&mut self, tx: &Transaction<'entry>) -> eyre::Result<()> {
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
