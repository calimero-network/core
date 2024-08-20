use crate::iter::{DBIter, Iter, Structured};
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
            inner: self.inner.iter::<K>()?,
            shadow: &self.shadow,
            shadow_iter: None,
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
    inner: Iter<'a, Structured<K>>,
    shadow: &'a Transaction<'b>,
    shadow_iter: Option<tx::ColRange<'a, 'b>>,
    value: Option<Slice<'a>>,
}

impl<'a, 'b, K: AsKeyParts + FromKeyParts> DBIter for TemporalIterator<'a, 'b, K> {
    fn seek(&mut self, key: Slice) -> eyre::Result<Option<Slice>> {
        self.shadow_iter = Some(self.shadow.col_iter(K::column(), Some(&key)));
        self.inner.seek(key)
    }

    fn next(&mut self) -> eyre::Result<Option<Slice>> {
        self.value = None;

        loop {
            // safety: Slice doesn't mutably borrow self
            let other = unsafe { &mut *(&mut self.inner as *mut Iter<'a, Structured<K>>) };

            let Some(key) = other.next()? else {
                break;
            };

            match self.shadow.raw_get(K::column(), &key) {
                Some(Operation::Delete) => continue,
                Some(Operation::Put { value }) => self.value = Some(value.into()),
                None => {}
            }

            return Ok(Some(key));
        }

        let shadow_iter = self
            .shadow_iter
            .get_or_insert_with(|| self.shadow.col_iter(K::column(), None));

        loop {
            if let Some((key, op)) = shadow_iter.next() {
                match op {
                    Operation::Delete => continue,
                    Operation::Put { value } => self.value = Some(value.into()),
                }

                return Ok(Some(key.into()));
            }

            return Ok(None);
        }
    }

    fn read(&self) -> eyre::Result<Slice> {
        if let Some(value) = &self.value {
            return Ok(value.into());
        };

        self.inner.read()
    }
}
