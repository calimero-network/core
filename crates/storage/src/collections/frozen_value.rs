//! A wrapper type for immutable values stored in FrozenStorage.

use crate::collections::crdt_meta::{MergeError, Mergeable};
use borsh::io::{Read, Write};
use borsh::{BorshDeserialize, BorshSerialize};
use core::ops::Deref;

/// A wrapper for frozen (immutable) values.
///
/// This struct implements `Mergeable` with an empty `merge` function,
/// satisfying the CRDT trait bounds for a value that cannot be changed.
/// It uses "transparent" Borsh implementation so that it serializes exactly as
/// its inner value `T`.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrozenValue<T>(pub T);

// Frozen values are immutable and never re-keyed (#D5: RekeyTarget supertrait).
impl<T: 'static> crate::collections::rekey::RekeyTarget for FrozenValue<T> {
    fn rekey_relative_to(&mut self, _parent_id: crate::address::Id) {}
}

#[diagnostic::do_not_recommend]
impl<T: 'static> Mergeable for FrozenValue<T> {
    /// Merging a frozen value does nothing, as it is immutable.
    fn merge(&mut self, _other: &Self) -> Result<(), MergeError> {
        // Do nothing.
        Ok(())
    }
}

// Manual BorshSerialize impl to be transparent
impl<T: BorshSerialize> BorshSerialize for FrozenValue<T> {
    fn serialize<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        // Serialize the inner value directly
        self.0.serialize(writer)
    }
}

// Manual BorshDeserialize impl to be transparent
impl<T: BorshDeserialize> BorshDeserialize for FrozenValue<T> {
    fn deserialize_reader<R: Read>(reader: &mut R) -> std::io::Result<Self> {
        // Deserialize the inner value directly and wrap it
        Ok(Self(T::deserialize_reader(reader)?))
    }
}

/// Allows direct read-only access to the inner value.
impl<T> Deref for FrozenValue<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Allows borrowing the inner value.
impl<T> AsRef<T> for FrozenValue<T> {
    fn as_ref(&self) -> &T {
        &self.0
    }
}

/// Allows creating a FrozenValue from its inner value.
impl<T> From<T> for FrozenValue<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}
