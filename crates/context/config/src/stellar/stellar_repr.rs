use core::borrow::Borrow;
use core::ops::Deref;
use core::fmt;

use crate::repr::{self, ReprBytes};

#[derive(Clone)]
pub struct StellarRepr<T> {
    inner: T,
}

impl<T: fmt::Debug> fmt::Debug for StellarRepr<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StellarRepr")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T> StellarRepr<T> {
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T> Deref for StellarRepr<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> Borrow<T> for StellarRepr<T> {
    fn borrow(&self) -> &T {
        &self.inner
    }
}

impl<T: ReprBytes> ReprBytes for StellarRepr<T> {
    type EncodeBytes<'a> = T::EncodeBytes<'a> where T: 'a;
    type DecodeBytes = T::DecodeBytes;
    type Error = T::Error;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.inner.as_bytes()
    }

    fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Result<usize, bs58::decode::Error>,
    {
        T::from_bytes(f).map(Self::new)
    }
}
