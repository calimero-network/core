use eyre::Report;
use thiserror::Error as ThisError;

use crate::entry::{Codec, Entry};
use crate::iter::{Iter, Structured};
use crate::key::FromKeyParts;
use crate::layer::{Layer, ReadLayer, WriteLayer};

#[derive(Debug)]
pub struct Handle<L> {
    pub(crate) inner: L,
}

impl<L: Layer> Handle<L> {
    pub const fn new(inner: L) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> L {
        self.inner
    }
}

#[derive(Debug, ThisError)]
pub enum HandleError<E> {
    #[error(transparent)]
    LayerError(#[from] Report),
    #[error(transparent)]
    CodecError(E),
}

// todo! detach 'a from EntryError
type EntryError<'a, E> =
    HandleError<<<E as Entry>::Codec as Codec<'a, <E as Entry>::DataType<'a>>>::Error>;

impl<L: ReadLayer> Handle<L> {
    pub fn has<E: Entry>(&self, entry: &E) -> Result<bool, EntryError<'_, E>> {
        Ok(self.inner.has(entry.key())?)
    }

    pub fn get<E: Entry>(&self, entry: &E) -> Result<Option<E::DataType<'_>>, EntryError<'_, E>> {
        match self.inner.get(entry.key())? {
            Some(value) => Ok(Some(
                E::Codec::decode(value).map_err(HandleError::CodecError)?,
            )),
            None => Ok(None),
        }
    }

    // TODO: We should consider returning Iterator here.
    #[expect(
        clippy::iter_not_returning_iterator,
        reason = "TODO: This should be implemented"
    )]
    #[expect(clippy::type_complexity, reason = "Acceptable here")]
    pub fn iter<E: Entry<Key: FromKeyParts>>(
        &self,
    ) -> Result<
        Iter<'_, Structured<E::Key>, Structured<(E::DataType<'_>, E::Codec)>>,
        EntryError<'_, E>,
    > {
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
                E::Codec::encode(value).map_err(HandleError::CodecError)?,
            )
            .map_err(HandleError::LayerError)
    }

    pub fn delete<E: Entry>(&'a mut self, entry: &'a E) -> Result<(), EntryError<'a, E>> {
        self.inner
            .delete(entry.key())
            .map_err(HandleError::LayerError)
    }
}

// todo! consider & experiment with restoring {Read,Write}Layer for StoreHandle
