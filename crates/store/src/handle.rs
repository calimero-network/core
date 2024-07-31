use thiserror::Error;

use crate::entry::{Codec, Entry};
use crate::iter::{Iter, Structured};
use crate::key::FromKeyParts;
use crate::layer::{Layer, ReadLayer, WriteLayer};

pub struct Handle<L> {
    pub(crate) inner: L,
}

impl<L: Layer> Handle<L> {
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

type EntryError<'a, E> =
    Error<<<E as Entry>::Codec as Codec<'a, <E as Entry>::DataType<'a>>>::Error>;

impl<L: ReadLayer> Handle<L> {
    pub fn has<E: Entry>(&self, entry: &E) -> Result<bool, EntryError<E>> {
        Ok(self.inner.has(entry.key())?)
    }

    pub fn get<E: Entry>(&self, entry: &E) -> Result<Option<E::DataType<'_>>, EntryError<E>> {
        match self.inner.get(entry.key())? {
            Some(value) => Ok(Some(E::Codec::decode(value).map_err(Error::CodecError)?)),
            None => Ok(None),
        }
    }

    pub fn iter<E: Entry<Key: FromKeyParts>>(
        &self,
    ) -> Result<Iter<Structured<E::Key>, Structured<(E::DataType<'_>, E::Codec)>>, EntryError<E>>
    {
        Ok(self.inner.iter()?.structured_value())
    }
}

impl<'a, L: WriteLayer<'a>> Handle<L> {
    pub fn put<'b, E: Entry>(
        &'a mut self,
        entry: &'a E,
        value: &'a E::DataType<'b>,
    ) -> Result<(), EntryError<'b, E>> {
        self.inner
            .put(
                entry.key(),
                E::Codec::encode(value).map_err(Error::CodecError)?,
            )
            .map_err(Error::LayerError)
    }

    pub fn delete<E: Entry>(&'a mut self, entry: &'a E) -> Result<(), EntryError<E>> {
        self.inner.delete(entry.key()).map_err(Error::LayerError)
    }
}

// todo! consider & experiment with restoring {Read,Write}Layer for StoreHandle
