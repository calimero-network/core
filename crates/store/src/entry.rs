use std::marker::PhantomData;
use std::ops::Deref;

use crate::key::AsKeyParts;
use crate::slice::Slice;

pub trait DataType<'a>: Sized {
    type Error;

    // todo! change to &'a [u8]
    fn from_slice(slice: Slice<'a>) -> Result<Self, Self::Error>;
    fn as_slice(&'a self) -> Result<Slice<'a>, Self::Error>;
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

pub struct View<T, C> {
    inner: T,
    _priv: PhantomData<C>,
}

pub trait Codec<T> {
    type Error;

    fn encode<'a>(value: &'a T) -> Result<Slice<'a>, Self::Error>;
    fn decode(bytes: &[u8]) -> Result<T, Self::Error>;
}

impl<T, C: Codec<T>> View<T, C> {
    pub fn new(value: T) -> Self {
        Self {
            inner: value,
            _priv: PhantomData,
        }
    }

    pub fn value(self) -> T {
        self.inner
    }
}

impl<T, C> Deref for View<T, C> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'a, T, C: Codec<T>> DataType<'a> for View<T, C> {
    type Error = C::Error;

    fn from_slice(slice: Slice<'_>) -> Result<Self, Self::Error> {
        Ok(Self::new(C::decode(&slice)?))
    }

    fn as_slice(&'a self) -> Result<Slice<'a>, Self::Error> {
        C::encode(&self.inner).map(Into::into)
    }
}

#[cfg(feature = "serde")]
pub enum Json {}

#[cfg(feature = "serde")]
impl<T> Codec<T> for Json
where
    // todo! investigate Deserialize<'a> when DataType<'a>::from_slice(&'a [u8]) is implemented
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    type Error = serde_json::Error;

    fn encode<'a>(value: &'a T) -> Result<Slice<'a>, Self::Error> {
        serde_json::to_vec(&value).map(Into::into)
    }

    fn decode(bytes: &[u8]) -> Result<T, Self::Error> {
        serde_json::from_slice(bytes)
    }
}

#[cfg(feature = "borsh")]
pub enum Borsh {}

#[cfg(feature = "borsh")]
impl<T> Codec<T> for Borsh
where
    T: borsh::BorshSerialize + borsh::BorshDeserialize,
{
    type Error = std::io::Error;

    fn encode<'a>(value: &'a T) -> Result<Slice<'a>, Self::Error> {
        borsh::to_vec(&value).map(Into::into)
    }

    fn decode(bytes: &[u8]) -> Result<T, Self::Error> {
        borsh::from_slice(bytes)
    }
}
