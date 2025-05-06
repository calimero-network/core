use core::convert::Infallible;
use core::fmt::{self, Debug, Formatter};

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::blobs::BlobId as PrimitiveBlobId;
use generic_array::typenum::U32;

use crate::db::Column;
use crate::key::component::KeyComponent;
use crate::key::{AsKeyParts, FromKeyParts, Key};

#[derive(Clone, Copy, Debug)]
pub struct BlobId;

impl KeyComponent for BlobId {
    type LEN = U32;
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct BlobMeta(Key<BlobId>);

impl BlobMeta {
    #[must_use]
    pub fn new(blob_id: PrimitiveBlobId) -> Self {
        Self(Key((*blob_id).into()))
    }

    #[must_use]
    pub fn blob_id(&self) -> PrimitiveBlobId {
        (*AsRef::<[_; 32]>::as_ref(&self.0)).into()
    }
}

impl AsKeyParts for BlobMeta {
    type Components = (BlobId,);

    fn column() -> Column {
        Column::Blobs
    }

    fn as_key(&self) -> &Key<Self::Components> {
        (&self.0).into()
    }
}

impl FromKeyParts for BlobMeta {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(*<&_>::from(&parts)))
    }
}

impl Debug for BlobMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlobMeta")
            .field("id", &self.blob_id())
            .finish()
    }
}
