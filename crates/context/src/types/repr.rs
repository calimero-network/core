//! Representation wrapper types for safe transmutation

use std::marker::PhantomData;
use thiserror::Error;

/// A wrapper type that allows safe transmutation of data
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct Repr<T> {
    inner: T,
}

impl<T> Repr<T> {
    /// Create a new Repr wrapper
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    /// Get the inner value
    pub fn into_inner(self) -> T {
        self.inner
    }

    /// Get a reference to the inner value
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Get a mutable reference to the inner value
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Transmute to another type (unsafe but controlled)
    pub fn rt<U>(self) -> Result<U, ReprError>
    where
        T: ReprTransmute<U>,
    {
        T::transmute(self.inner)
    }
}

impl<T> std::ops::Deref for Repr<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> std::ops::DerefMut for Repr<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// Trait for safe transmutation between types
pub trait ReprTransmute<T> {
    fn transmute(self) -> Result<T, ReprError>;
}

/// Error type for representation operations
#[derive(Debug, Error)]
pub enum ReprError {
    #[error("transmutation failed: {0}")]
    TransmutationFailed(String),
}

// Implement ReprTransmute for common types
impl ReprTransmute<[u8; 32]> for [u8; 32] {
    fn transmute(self) -> Result<[u8; 32], ReprError> {
        Ok(self)
    }
}

impl ReprTransmute<Vec<u8>> for Vec<u8> {
    fn transmute(self) -> Result<Vec<u8>, ReprError> {
        Ok(self)
    }
}

impl ReprTransmute<String> for String {
    fn transmute(self) -> Result<String, ReprError> {
        Ok(self)
    }
}

// Add more implementations as needed
