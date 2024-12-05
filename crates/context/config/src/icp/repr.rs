use std::borrow::Borrow;
use std::ops::Deref;

use bs58::decode::Result as Bs58Result;
use candid::CandidType;
use serde::Deserialize;

use crate::repr::{self, ReprBytes};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(transparent)]
#[repr(transparent)]
pub struct ICRepr<T> {
    #[serde(bound = "for<'a> T: ReprBytes<DecodeBytes: Deserialize<'a>>")]
    #[serde(deserialize_with = "repr_deserialize")]
    inner: T,
}

impl<T> ICRepr<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }
}

impl<T> Deref for ICRepr<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> Borrow<T> for ICRepr<T> {
    fn borrow(&self) -> &T {
        &self.inner
    }
}

impl<T: ReprBytes> CandidType for ICRepr<T>
where
    for<'a> T::EncodeBytes<'a>: CandidType,
{
    fn _ty() -> candid::types::Type {
        <T::EncodeBytes<'_> as CandidType>::_ty()
    }

    fn idl_serialize<S>(&self, serializer: S) -> Result<(), S::Error>
    where
        S: candid::types::Serializer,
    {
        self.inner.as_bytes().idl_serialize(serializer)
    }
}

fn repr_deserialize<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    for<'a> T: ReprBytes<DecodeBytes: Deserialize<'a>>,
    D: serde::Deserializer<'de>,
{
    let bytes = T::DecodeBytes::deserialize(deserializer)?;

    T::from_bytes(|buf| {
        *buf = bytes;

        Ok(buf.as_ref().len())
    })
    .map_err(serde::de::Error::custom)
}

impl<T: ReprBytes> ReprBytes for ICRepr<T> {
    type EncodeBytes<'a>
        = T::EncodeBytes<'a>
    where
        T: 'a;
    type DecodeBytes = T::DecodeBytes;

    type Error = T::Error;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.inner.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
    {
        T::from_bytes(f).map(Self::new)
    }
}
