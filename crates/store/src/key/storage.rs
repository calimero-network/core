//! Storage key.
//!
//! This module contains the storage key type and related functionality. It is
//! used to identify records in the storage system, and to provide a means of
//! accessing and manipulating them.
//!

use borsh::{BorshDeserialize, BorshSerialize};
use generic_array::typenum::U16;
use generic_array::GenericArray;

use super::component::KeyComponent;
use crate::db::Column;
use crate::key::{AsKeyParts, FromKeyParts, Key};

/// The identifier for a storage record, based around a UUID.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StorageId;

impl KeyComponent for StorageId {
    // UUIDs are 16 bytes long.
    type LEN = U16;
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "borsh", derive(BorshSerialize, BorshDeserialize))]
pub struct Storage(Key<(StorageId,)>);

impl Storage {
    /// Creates a new instance of the [`Storage`] key.
    ///
    /// # Parameters
    ///
    /// * `id` - The unique identifier for the storage record. This is a UUID.
    ///
    #[must_use]
    pub fn new(id: [u8; 16]) -> Self {
        Self(Key(GenericArray::from(id)))
    }

    /// The unique identifier for the storage record. This is a UUID.
    #[must_use]
    pub fn id(&self) -> [u8; 16] {
        *self.0.as_ref()
    }
}

impl AsKeyParts for Storage {
    type Components = (StorageId,);

    fn column() -> Column {
        // TODO: Check if this is the most appropriate column type.
        Column::Generic
    }

    fn as_key(&self) -> &Key<Self::Components> {
        &self.0
    }
}

impl From<Storage> for [u8; 16] {
    fn from(storage: Storage) -> Self {
        *storage.0.as_ref()
    }
}

impl From<[u8; 16]> for Storage {
    fn from(bytes: [u8; 16]) -> Self {
        Self(Key(GenericArray::from(bytes)))
    }
}

impl From<&[u8; 16]> for Storage {
    fn from(bytes: &[u8; 16]) -> Self {
        Self(Key(GenericArray::from(*bytes)))
    }
}

impl FromKeyParts for Storage {
    type Error = ();

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error> {
        Ok(Self(parts))
    }
}
