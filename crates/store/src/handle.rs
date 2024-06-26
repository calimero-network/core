use thiserror::Error;

use crate::entry::{DataType, Entry};
use crate::iter::{Iter, Structured};
use crate::key::FromKeyParts;
use crate::layer::{read_only, temporal, Layer, ReadLayer, WriteLayer};
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

impl<'k, L: ReadLayer<'k>> StoreHandle<L> {
    pub fn read_only(&'k self) -> read_only::ReadOnly<'k, L> {
        read_only::ReadOnly::new(&self.inner)
    }
}

impl<'base, 'k, 'v, L: WriteLayer<'k, 'v>> StoreHandle<L> {
    pub fn temporal(&'base mut self) -> temporal::Temporal<'base, 'k, 'v, L> {
        temporal::Temporal::new(&mut self.inner)
    }
}

#[derive(Debug, Error)]
pub enum Error<E: DataType> {
    LayerError(eyre::Report),
    CodecError(E::Error),
}

impl<E: DataType> From<eyre::Report> for Error<E> {
    fn from(err: eyre::Report) -> Self {
        Self::LayerError(err)
    }
}

impl<'k, L: ReadLayer<'k>> StoreHandle<L> {
    pub fn has<E: Entry>(&self, entry: &'k E) -> Result<bool, Error<E::DataType>> {
        Ok(self.inner.has(entry.key())?)
    }

    pub fn get<E: Entry>(&self, entry: &'k E) -> Result<Option<E::DataType>, Error<E::DataType>> {
        match self.inner.get(entry.key())? {
            Some(value) => Ok(Some(
                E::DataType::from_slice(value).map_err(Error::CodecError)?,
            )),
            None => Ok(None),
        }
    }

    pub fn iter<E: Entry<Key: FromKeyParts>>(
        &'k self,
        start: &'k E,
    ) -> Result<Iter<Structured<E::Key>, Structured<E::DataType>>, Error<E::DataType>> {
        Ok(self.inner.iter(start.key())?.structured_value())
    }
}
