use crate::key::AsKeyParts;
use crate::slice::Slice;

pub trait Entry {
    type Key: AsKeyParts;
    type Codec: for<'a> Codec<'a, Self::DataType<'a>>;
    type DataType<'a>;

    fn key(&self) -> &Self::Key;

    // each entry should be able to define what
    // happens when it's operated on wrt storage
    // for example: to ref dec one of it's fields
    // when it's changed, for example
    // read old state, check if it's changed, decrement
    // the referent entry
}

pub trait Codec<'a, T> {
    type Error;

    fn encode(value: &T) -> Result<Slice<'_>, Self::Error>;
    fn decode(bytes: Slice<'a>) -> Result<T, Self::Error>;
}

#[derive(Clone, Copy)]
pub enum Identity {}

impl<'a, T, E> Codec<'a, T> for Identity
where
    T: AsRef<[u8]> + TryFrom<Slice<'a>, Error = E>,
{
    type Error = E;

    fn encode(value: &T) -> Result<Slice<'_>, Self::Error> {
        Ok(value.into())
    }

    fn decode(bytes: Slice<'a>) -> Result<T, Self::Error> {
        bytes.try_into()
    }
}

#[cfg(feature = "serde")]
#[derive(Clone, Copy)]
pub enum Json {}

#[cfg(feature = "serde")]
impl<T> Codec<'_, T> for Json
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    type Error = serde_json::Error;

    fn encode(value: &T) -> Result<Slice<'_>, Self::Error> {
        serde_json::to_vec(value).map(Into::into)
    }

    fn decode(bytes: Slice<'_>) -> Result<T, Self::Error> {
        serde_json::from_slice(&bytes)
    }
}

#[cfg(feature = "borsh")]
#[derive(Clone, Copy)]
pub enum Borsh {}

#[cfg(feature = "borsh")]
impl<T> Codec<'_, T> for Borsh
where
    T: borsh::BorshSerialize + borsh::BorshDeserialize,
{
    type Error = std::io::Error;

    fn encode(value: &T) -> Result<Slice<'_>, Self::Error> {
        borsh::to_vec(&value).map(Into::into)
    }

    fn decode(bytes: Slice<'_>) -> Result<T, Self::Error> {
        borsh::from_slice(&bytes)
    }
}
