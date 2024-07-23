use crate::iter::{DBIter, Iter, Structured};
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
    fn has<K: AsKeyParts>(&'base self, key: &'base K) -> eyre::Result<bool> {
        match self.shadow.get(key) {
            Some(Operation::Delete) => Ok(false),
            Some(Operation::Put { .. }) => Ok(true),
            None => self.inner.has(key),
        }
    }

    fn get<K: AsKeyParts>(&'base self, key: &'base K) -> eyre::Result<Option<Slice<'base>>> {
        match self.shadow.get(key) {
            Some(Operation::Delete) => Ok(None),
            Some(Operation::Put { value }) => Ok(Some(value.into())),
            None => self.inner.get(key),
        }
    }

    fn iter<K: FromKeyParts>(&'base self) -> eyre::Result<Iter<Structured<K>>> {
        // todo! track lifetimes

        Ok(Iter::new(TemporalIterator {
            inner: self.inner.iter::<K>()?,
            shadow: &self.shadow,
        }))
    }
}

impl<'base, 'entry, L> WriteLayer<'entry> for Temporal<'base, 'entry, L>
where
    L: WriteLayer<'entry>,
{
    fn put<K: AsKeyParts>(&mut self, key: &'entry K, value: Slice<'entry>) -> eyre::Result<()> {
        self.shadow.put(key, value);

        Ok(())
    }

    fn delete<K: AsKeyParts>(&mut self, key: &'entry K) -> eyre::Result<()> {
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

struct TemporalIterator<'a, 'b, K> {
    inner: Iter<'a, Structured<K>>,
    shadow: &'a Transaction<'b>,
}

impl<'a, 'b, K: AsKeyParts + FromKeyParts> DBIter for TemporalIterator<'a, 'b, K> {
    fn seek(&mut self, key: Slice) -> eyre::Result<()> {
        DBIter::seek(&mut self.inner, key)
    }

    fn next(&mut self) -> eyre::Result<Option<Slice>> {
        loop {
            // safety: Slice doesn't mutably borrow self
            let other = unsafe { &mut *(&mut self.inner as *mut Iter<'a, Structured<K>>) };

            let Some(key) = other.next()? else {
                return Ok(None);
            };

            match self.shadow.raw_get(K::column(), &key) {
                Some(Operation::Put { value }) => return Ok(Some(value.as_ref().into())),
                Some(Operation::Delete) => continue,
                None => return Ok(Some(key)),
            }
        }
    }

    fn read(&self) -> eyre::Result<Slice> {
        self.inner.read()
    }
}
