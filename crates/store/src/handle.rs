use thiserror::Error;

use crate::entry::{DataType, Entry};
use crate::iter::{Iter, Structured};
use crate::key::FromKeyParts;
use crate::layer::{Layer, ReadLayer, WriteLayer};
use crate::Store;

pub struct StoreHandle<L = Store> {
    pub(crate) inner: L,
}

impl<L: Layer> StoreHandle<L> {
    pub fn new(inner: L) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> L {
        self.inner
    }
}

#[derive(Error, Debug)]
pub enum Error<E> {
    #[error(transparent)]
    LayerError(#[from] eyre::Report),
    #[error(transparent)]
    CodecError(E),
}

type EntryError<'a, E> = Error<<<E as Entry>::DataType<'a> as DataType<'a>>::Error>;

impl<'k, L: ReadLayer<'k>> StoreHandle<L> {
    pub fn has<E: Entry>(&self, entry: &'k E) -> Result<bool, EntryError<E>> {
        Ok(self.inner.has(entry.key())?)
    }

    pub fn get<E: Entry>(&self, entry: &'k E) -> Result<Option<E::DataType<'_>>, EntryError<E>> {
        match self.inner.get(entry.key())? {
            Some(value) => Ok(Some(
                E::DataType::from_slice(value).map_err(Error::CodecError)?,
            )),
            None => Ok(None),
        }
    }

    pub fn iter<E: Entry<Key: FromKeyParts>>(
        &self,
        start: &'k E,
    ) -> Result<Iter<Structured<E::Key>, Structured<E::DataType<'_>>>, EntryError<E>> {
        Ok(self.inner.iter(start.key())?.structured_value())
    }
}

impl<'k, 'v, L: WriteLayer<'k, 'v>> StoreHandle<L> {
    pub fn put<E: Entry>(
        &'v mut self,
        entry: &'k E,
        value: &'v E::DataType<'v>,
    ) -> Result<(), EntryError<E>> {
        self.inner
            .put(entry.key(), value.as_slice().map_err(Error::CodecError)?)
            .map_err(Error::LayerError)
    }

    pub fn delete<E: Entry>(&mut self, entry: &'k E) -> Result<(), EntryError<E>> {
        self.inner.delete(entry.key()).map_err(Error::LayerError)
    }
}

// todo! consider & experiment with restoring {Read,Write}Layer for StoreHandle
