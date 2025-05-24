use core::ptr;

use eyre::Result as EyreResult;

use crate::iter::{DBIter, Iter, Structured};
use crate::key::{AsKeyParts, FromKeyParts};
use crate::layer::{Layer, ReadLayer, WriteLayer};
use crate::slice::Slice;
use crate::tx::{self, Operation, Transaction};

#[derive(Debug)]
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

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shadow.is_empty()
    }
}

impl<L> Layer for Temporal<'_, '_, L>
where
    L: Layer,
{
    type Base = L;
}

impl<L> ReadLayer for Temporal<'_, '_, L>
where
    L: ReadLayer,
{
    fn has<K: AsKeyParts>(&self, key: &K) -> EyreResult<bool> {
        match self.shadow.get(key) {
            Some(Operation::Delete) => Ok(false),
            Some(Operation::Put { .. }) => Ok(true),
            None => self.inner.has(key),
        }
    }

    fn get<K: AsKeyParts>(&self, key: &K) -> EyreResult<Option<Slice<'_>>> {
        match self.shadow.get(key) {
            Some(Operation::Delete) => Ok(None),
            Some(Operation::Put { value }) => Ok(Some(value.into())),
            None => self.inner.get(key),
        }
    }

    fn iter<K: FromKeyParts>(&self) -> EyreResult<Iter<'_, Structured<K>>> {
        Ok(Iter::new(TemporalIterator {
            inner: self.inner.iter::<K>()?,
            shadow: &self.shadow,
            shadow_iter: None,
            value: None,
        }))
    }
}

impl<'entry, L> WriteLayer<'entry> for Temporal<'_, 'entry, L>
where
    L: WriteLayer<'entry>,
{
    fn put<K: AsKeyParts>(&mut self, key: &'entry K, value: Slice<'entry>) -> EyreResult<()> {
        self.shadow.put(key, value);

        Ok(())
    }

    fn delete<K: AsKeyParts>(&mut self, key: &'entry K) -> EyreResult<()> {
        self.shadow.delete(key);

        Ok(())
    }

    fn apply(&mut self, tx: &Transaction<'entry>) -> EyreResult<()> {
        self.shadow.merge(tx);

        Ok(())
    }

    fn commit(&mut self) -> EyreResult<()> {
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

impl<'a, K: AsKeyParts + FromKeyParts> DBIter for TemporalIterator<'a, '_, K> {
    fn seek(&mut self, key: Slice<'_>) -> EyreResult<Option<Slice<'_>>> {
        self.shadow_iter = Some(self.shadow.col_iter(K::column(), Some(&key)));
        self.inner.seek(key)
    }

    fn next(&mut self) -> EyreResult<Option<Slice<'_>>> {
        self.value = None;

        loop {
            // safety: Slice doesn't mutably borrow self
            let other = unsafe { &mut *ptr::from_mut::<Iter<'a, Structured<K>>>(&mut self.inner) };

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
                // todo! if key is in inner, we've already seen it, continue
                match op {
                    Operation::Delete => continue,
                    Operation::Put { value } => self.value = Some(value.into()),
                }

                return Ok(Some(key));
            }

            return Ok(None);
        }
    }

    fn read(&self) -> EyreResult<Slice<'_>> {
        if let Some(value) = &self.value {
            return Ok(value.into());
        };

        self.inner.read()
    }
}
