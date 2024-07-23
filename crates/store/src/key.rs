use std::fmt;

use generic_array::typenum::Const;
use generic_array::{GenericArray, IntoArrayLength};

use crate::db::Column;
use crate::slice::Slice;

mod application;
mod component;
mod context;
mod generic;

pub use application::ApplicationMeta;
use component::KeyComponents;
pub use context::{ContextIdentity, ContextMeta, ContextState, ContextTransaction};
pub use generic::Generic;

pub struct Key<T: KeyComponents>(GenericArray<u8, T::LEN>);

impl<T: KeyComponents> Copy for Key<T> where GenericArray<u8, T::LEN>: Copy {}
impl<T: KeyComponents> Clone for Key<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: KeyComponents> fmt::Debug for Key<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Key").field(&self.0).finish()
    }
}

impl<T: KeyComponents> Eq for Key<T> where GenericArray<u8, T::LEN>: Eq {}
impl<T: KeyComponents> PartialEq for Key<T>
where
    GenericArray<u8, T::LEN>: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: KeyComponents> Ord for Key<T>
where
    GenericArray<u8, T::LEN>: Ord,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl<T: KeyComponents> PartialOrd for Key<T>
where
    GenericArray<u8, T::LEN>: PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(&other.0)
    }
}

impl<T: KeyComponents> Key<T> {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn as_slice(&self) -> Slice {
        self.as_bytes().into()
    }

    pub(crate) fn try_from_slice(slice: Slice) -> Option<Self> {
        let bytes = slice.as_ref();

        (bytes.len() == GenericArray::<u8, T::LEN>::len()).then_some(())?;

        let mut key = GenericArray::default();

        key.copy_from_slice(bytes);

        Some(Self(key))
    }
}

impl<T: KeyComponents, const N: usize> AsRef<[u8; N]> for Key<T>
where
    Const<N>: IntoArrayLength<ArrayLength = T::LEN>,
{
    fn as_ref(&self) -> &[u8; N] {
        self.0.as_ref()
    }
}

impl<T> From<&Key<T>> for &Key<(T,)>
where
    T: KeyComponents,
    (T,): KeyComponents<LEN = T::LEN>,
{
    fn from(key: &Key<T>) -> Self {
        unsafe { &*(key as *const _ as *const _) }
    }
}

impl<T> From<&Key<(T,)>> for &Key<T>
where
    T: KeyComponents,
    (T,): KeyComponents<LEN = T::LEN>,
{
    fn from(key: &Key<(T,)>) -> Self {
        unsafe { &*(key as *const _ as *const _) }
    }
}

pub trait AsKeyParts: Copy {
    // KeyParts is Sealed so far as KeyComponents stays private
    type Components: KeyComponents;

    fn column() -> Column;
    fn as_key(&self) -> &Key<Self::Components>;
}

pub trait FromKeyParts: AsKeyParts {
    type Error;

    fn try_from_parts(parts: Key<Self::Components>) -> Result<Self, Self::Error>;
}

#[cfg(feature = "borsh")]
const _: () = {
    use std::io;

    use borsh::{BorshDeserialize, BorshSerialize};

    impl<T: KeyComponents> BorshSerialize for Key<T> {
        fn serialize<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
            writer.write_all(&self.0)
        }
    }

    impl<T: KeyComponents> BorshDeserialize for Key<T> {
        fn deserialize_reader<R: io::Read>(reader: &mut R) -> io::Result<Self> {
            let mut key = GenericArray::default();

            reader.read_exact(&mut key)?;

            Ok(Self(key))
        }
    }
};
