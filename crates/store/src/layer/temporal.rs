use crate::iter::{DBIter, FusedIter, Iter, Structured};
use crate::key::{AsKeyParts, FromKeyParts};
use crate::layer::{Layer, ReadLayer, WriteLayer};
use crate::slice::Slice;
use crate::tx::{self, Operation, Transaction};

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

impl<'base, 'entry, L> ReadLayer for Temporal<'base, 'entry, L>
where
    L: ReadLayer,
{
    fn has<K: AsKeyParts>(&self, key: &K) -> eyre::Result<bool> {
        match self.shadow.get(key) {
            Some(Operation::Delete) => Ok(false),
            Some(Operation::Put { .. }) => Ok(true),
            None => self.inner.has(key),
        }
    }

    fn get<K: AsKeyParts>(&self, key: &K) -> eyre::Result<Option<Slice>> {
        match self.shadow.get(key) {
            Some(Operation::Delete) => Ok(None),
            Some(Operation::Put { value }) => Ok(Some(value.into())),
            None => self.inner.get(key),
        }
    }

    fn iter<K: FromKeyParts>(&self) -> eyre::Result<Iter<Structured<K>>> {
        Ok(Iter::new(TemporalIterator {
            inner: FusedIter::new(self.inner.iter::<K>()?),
            shadow: &self.shadow,
            range: None,
            value: None,
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
    inner: FusedIter<Iter<'a, Structured<K>>>,
    shadow: &'a Transaction<'b>,
    range: Option<tx::Iter<'a, 'b>>,
    value: Option<Slice<'a>>,
}

impl<'a, 'b, K: AsKeyParts + FromKeyParts> DBIter for TemporalIterator<'a, 'b, K> {
    fn seek(&mut self, key: Slice) -> eyre::Result<Option<Slice>> {
        self.inner.seek(key)
    }

    fn next(&mut self) -> eyre::Result<Option<Slice>> {
        loop {
            self.value = None;

            // safety: Slice doesn't mutably borrow self
            let other =
                unsafe { &mut *(&mut self.inner as *mut FusedIter<Iter<'a, Structured<K>>>) };

            if let Some(key) = other.next()? {
                match self.shadow.raw_get(K::column(), &key) {
                    Some(Operation::Delete) => continue,
                    Some(Operation::Put { value }) => self.value = Some(value.into()),
                    None => {}
                }

                return Ok(Some(key));
            }

            let shadow_iter = self.range.get_or_insert_with(|| self.shadow.iter());

            if let Some((entry, op)) = shadow_iter.next() {
                match op {
                    Operation::Delete => continue,
                    Operation::Put { value } => self.value = Some(value.into()),
                }

                return Ok(Some(entry.key().into()));
            }

            return Ok(None);
        }
    }

    fn read(&self) -> eyre::Result<Slice> {
        if let Some(value) = &self.value {
            return Ok(value.into());
        };

        let Some(value) = self.inner.read()? else {
            eyre::bail!("missing value for iterator entry");
        };

        Ok(value)
    }
}
