use std::marker::PhantomData;
use std::ops::Deref;
use std::{fmt, io};

use bs58::decode::DecodeTarget;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{de, ser, Deserialize, Serialize};

#[derive(Eq, Copy, Clone, PartialEq)]
pub enum Bs58 {}

#[derive(Eq, Copy, Clone, PartialEq)]
pub enum Raw {}

mod private {
    pub trait Sealed {}
}

pub trait ReprFormat: private::Sealed {}

impl private::Sealed for Bs58 {}
impl ReprFormat for Bs58 {}

impl private::Sealed for Raw {}
impl ReprFormat for Raw {}

#[derive(Eq, Copy, Clone, PartialEq)]
pub struct Repr<T, F = Bs58> {
    data: T,
    _phantom: PhantomData<F>,
}

pub trait ReprBytes {
    type Bytes: AsRef<[u8]>;

    fn to_bytes(&self) -> Self::Bytes;
    fn from_bytes<F, E>(f: F) -> Option<Result<Self, E>>
    where
        F: FnOnce(&mut Self::Bytes) -> Option<E>,
        Self: Sized;
}

impl<T: ReprBytes, F: ReprFormat> From<T> for Repr<T, F> {
    fn from(data: T) -> Self {
        Repr {
            data,
            _phantom: PhantomData,
        }
    }
}

impl<T: ReprBytes> From<Repr<T, Bs58>> for Repr<T, Raw> {
    fn from(repr: Repr<T, Bs58>) -> Self {
        Repr {
            data: repr.data,
            _phantom: PhantomData,
        }
    }
}

impl<T, F> Deref for Repr<T, F> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T: Default + ReprBytes, F: ReprFormat> Default for Repr<T, F> {
    fn default() -> Self {
        Repr::from(T::default())
    }
}

impl<T: fmt::Debug, F> fmt::Debug for Repr<T, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.data.fmt(f)
    }
}

impl<T: ReprBytes> Serialize for Repr<T, Bs58> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        let bytes = self.data.to_bytes();
        let encoded = bs58::encode(bytes).into_string();
        serializer.serialize_str(&encoded)
    }
}

impl<'de, T: ReprBytes> Deserialize<'de> for Repr<T, Bs58>
where
    T::Bytes: DecodeTarget,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        let encoded = <String as Deserialize>::deserialize(deserializer)?;

        let data = match T::from_bytes(|bytes| bs58::decode(&encoded).onto(bytes).err()) {
            Some(data) => data.map_err(de::Error::custom)?,
            None => return Err(de::Error::custom("Invalid key")),
        };

        Ok(Repr::from(data))
    }
}

impl<T: ReprBytes> BorshSerialize for Repr<T, Raw>
where
    T::Bytes: BorshSerialize,
{
    fn serialize<W: io::Write>(&self, writer: &mut W) -> Result<(), io::Error> {
        self.data.to_bytes().serialize(writer)
    }
}

impl<T: ReprBytes> BorshDeserialize for Repr<T, Raw>
where
    T::Bytes: BorshDeserialize,
{
    fn deserialize_reader<R: io::Read>(reader: &mut R) -> io::Result<Self> {
        let bytes = T::Bytes::deserialize_reader(reader)?;

        let data = match T::from_bytes(|data| {
            *data = bytes;

            None::<()>
        }) {
            Some(data) => unsafe { data.unwrap_unchecked() },
            None => return Err(io::ErrorKind::InvalidData.into()),
        };

        Ok(Repr::from(data))
    }
}

impl<T: BorshSerialize> BorshSerialize for Repr<T, Bs58> {
    fn serialize<W: io::Write>(&self, writer: &mut W) -> Result<(), io::Error> {
        self.data.serialize(writer)
    }
}

impl<T: ReprBytes + BorshDeserialize> BorshDeserialize for Repr<T, Bs58> {
    fn deserialize_reader<R: io::Read>(reader: &mut R) -> io::Result<Self> {
        let data = T::deserialize_reader(reader)?;

        Ok(Repr::from(data))
    }
}
