use core::convert::Infallible;
use core::fmt::{self, Debug, Formatter};

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::blobs::BlobId as PrimitiveBlobId;
use calimero_primitives::context::ContextId as PrimitiveContextId;
use generic_array::sequence::Concat;
use generic_array::typenum::U32;
use generic_array::GenericArray;

use crate::db::Column;
use crate::key::component::KeyComponent;
use crate::key::context::ContextId;
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

/// Records that this node holds a blob for a context: added by the context's
/// app, uploaded for it, or fetched on its behalf. Presence is the signal.
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct BlobOwner(Key<(ContextId, BlobId)>);

impl BlobOwner {
    #[must_use]
    pub fn new(context_id: PrimitiveContextId, blob_id: PrimitiveBlobId) -> Self {
        Self(Key(
            GenericArray::from(*context_id).concat(GenericArray::from(*blob_id))
        ))
    }

    #[must_use]
    pub fn context_id(&self) -> PrimitiveContextId {
        let mut context_id = [0; 32];
        context_id.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[..32]);
        context_id.into()
    }

    #[must_use]
    pub fn blob_id(&self) -> PrimitiveBlobId {
        let mut blob_id = [0; 32];
        blob_id.copy_from_slice(&AsRef::<[_; 64]>::as_ref(&self.0)[32..]);
        blob_id.into()
    }
}

impl AsKeyParts for BlobOwner {
    type Components = (ContextId, BlobId);

    fn column() -> Column {
        Column::BlobOwner
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl FromKeyParts for BlobOwner {
    type Error = Infallible;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}

impl Debug for BlobOwner {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlobOwner")
            .field("context_id", &self.context_id())
            .field("blob_id", &self.blob_id())
            .finish()
    }
}
