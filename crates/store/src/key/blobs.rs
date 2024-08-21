use std::convert::Infallible;
use std::fmt;

use generic_array::typenum::U32;

use crate::db::Column;
use crate::key::component::KeyComponent;
use crate::key::{AsKeyParts, FromKeyParts, Key};

pub struct BlobId;

impl KeyComponent for BlobId {
    type LEN = U32;
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(
    feature = "borsh",
    derive(borsh::BorshSerialize, borsh::BorshDeserialize)
)]
pub struct BlobMeta(Key<BlobId>);

impl BlobMeta {
    pub fn new(blob_id: calimero_primitives::blobs::BlobId) -> Self {
        Self(Key((*blob_id).into()))
    }

    pub fn blob_id(&self) -> calimero_primitives::blobs::BlobId {
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

impl fmt::Debug for BlobMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlobMeta")
            .field("id", &self.blob_id())
            .finish()
    }
}
