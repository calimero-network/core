use std::marker::PhantomData;
use std::ops::Deref;

use crate::key::AsKeyParts;
use crate::slice::Slice;

mod private {
    pub trait Sealed {}
}

pub trait DataType<'a>: Sized + private::Sealed {
    type Error;

    fn to_slice(&self) -> Result<Slice, Self::Error>;
    fn from_slice(slice: Slice<'a>) -> Result<Self, Self::Error>;
}

pub trait Entry {
    type Key: AsKeyParts;
    type DataType<'a>: DataType<'a>;

    fn key(&self) -> &Self::Key;

    // each entry should be able to define what
    // happens when it's operated on wrt storage
    // for example: to ref dec one of it's fields
    // when it's changed, for example
    // read old state, check if it's changed, decrement
    // the referent entry
}

pub struct Value<T, C> {
    inner: T,
    _priv: PhantomData<C>,
}

pub trait Codec<'a, T> {
    type Error;

    fn encode(value: &T) -> Result<Slice, Self::Error>;
    fn decode(bytes: Slice<'a>) -> Result<T, Self::Error>;
}

impl<T, C> Value<T, C> {
    pub fn value(self) -> T {
        self.inner
    }
}

impl<'a, T, C: Codec<'a, T>> From<T> for Value<T, C> {
    fn from(value: T) -> Self {
        Self {
            inner: value,
            _priv: PhantomData,
        }
    }
}

impl<T, C> Deref for Value<T, C> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T, C> private::Sealed for Value<T, C> {}
impl<'a, T, C: Codec<'a, T>> DataType<'a> for Value<T, C> {
    type Error = C::Error;

    fn to_slice(&self) -> Result<Slice, Self::Error> {
        C::encode(&self.inner).map(Into::into)
    }

    fn from_slice(slice: Slice<'a>) -> Result<Self, Self::Error> {
        Ok(Self::from(C::decode(slice)?))
    }
}

pub enum Identity {}

impl<'a, T, E> Codec<'a, T> for Identity
where
    T: AsRef<[u8]> + TryFrom<Slice<'a>, Error = E> + 'a,
{
    type Error = E;

    fn encode(value: &T) -> Result<Slice, Self::Error> {
        Ok(value.into())
    }

    fn decode(bytes: Slice<'a>) -> Result<T, Self::Error> {
        bytes.try_into()
    }
}

#[cfg(feature = "serde")]
pub enum Json {}

#[cfg(feature = "serde")]
impl<'a, T> Codec<'a, T> for Json
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    type Error = serde_json::Error;

    fn encode(value: &T) -> Result<Slice, Self::Error> {
        serde_json::to_vec(value).map(Into::into)
    }

    fn decode(bytes: Slice<'a>) -> Result<T, Self::Error> {
        serde_json::from_slice(&bytes)
    }
}

#[cfg(feature = "borsh")]
pub enum Borsh {}

#[cfg(feature = "borsh")]
impl<'a, T> Codec<'a, T> for Borsh
where
    T: borsh::BorshSerialize + borsh::BorshDeserialize,
{
    type Error = std::io::Error;

    fn encode(value: &T) -> Result<Slice, Self::Error> {
        borsh::to_vec(&value).map(Into::into)
    }

    fn decode(bytes: Slice<'a>) -> Result<T, Self::Error> {
        borsh::from_slice(&bytes)
    }
}
