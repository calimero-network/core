use ed25519_dalek::{Signature, SigningKey, VerifyingKey};

use crate::errors::Error;
use crate::{Commitment, PublicKey};

pub trait AsKeyBytes {
    type Bytes: AsRef<[u8]>;

    fn as_key_bytes(&self) -> Self::Bytes;
    fn from_key_bytes(bytes: &[u8]) -> Result<Self, Error>
    where
        Self: Sized;
}

impl AsKeyBytes for PublicKey {
    type Bytes = [u8; 32];

    fn as_key_bytes(&self) -> Self::Bytes {
        *self.as_bytes()
    }

    fn from_key_bytes(bytes: &[u8]) -> Result<VerifyingKey, Error> {
        Ok(
            PublicKey::from_bytes(bytes.try_into().map_err(|_| Error::ByteSizeError)?)
                .map_err(|_| Error::ConversionError)?,
        )
    }
}

impl AsKeyBytes for SigningKey {
    type Bytes = [u8; 32];

    fn as_key_bytes(&self) -> Self::Bytes {
        *self.as_bytes()
    }

    fn from_key_bytes(bytes: &[u8]) -> Result<SigningKey, Error> {
        Ok(SigningKey::from_bytes(
            bytes.try_into().map_err(|_| Error::ByteSizeError)?,
        ))
    }
}

impl AsKeyBytes for Signature {
    type Bytes = [u8; 64];

    fn as_key_bytes(&self) -> Self::Bytes {
        self.to_bytes()
    }

    fn from_key_bytes(bytes: &[u8]) -> Result<Signature, Error> {
        Ok(Signature::from_bytes(
            bytes.try_into().map_err(|_| Error::ByteSizeError)?,
        ))
    }
}

impl AsKeyBytes for Commitment {
    type Bytes = [u8; 32];

    fn as_key_bytes(&self) -> Self::Bytes {
        *self
    }

    fn from_key_bytes(bytes: &[u8]) -> Result<[u8; 32], Error> {
        Ok(bytes.try_into().map_err(|_| Error::ByteSizeError)?)
    }
}
