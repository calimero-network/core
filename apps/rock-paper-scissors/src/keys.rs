use ed25519_dalek::{Signature, SigningKey, VerifyingKey};

use crate::Commitment;

pub trait KeyBytes {
    type Bytes: AsRef<[u8]> + Sized;

    fn as_key_bytes(&self) -> &Self::Bytes;
    fn from_key_bytes(bytes: Self::Bytes) -> Result<Self, Self::Bytes>
    where
        Self: Sized;
}

impl KeyBytes for VerifyingKey {
    type Bytes = [u8; 32];

    fn as_key_bytes(&self) -> &Self::Bytes {
        self.as_bytes()
    }

    fn from_key_bytes(bytes: [u8; 32]) -> Result<VerifyingKey, Self::Bytes> {
        match VerifyingKey::from_bytes(&bytes) {
            Ok(key) => Ok(key),
            Err(_err) => Err(bytes),
        }
    }
}

impl KeyBytes for SigningKey {
    type Bytes = [u8; 32];

    fn as_key_bytes(&self) -> &Self::Bytes {
        self.as_bytes()
    }

    fn from_key_bytes(bytes: [u8; 32]) -> Result<SigningKey, Self::Bytes> {
        Ok(SigningKey::from_bytes(&bytes))
    }
}

impl KeyBytes for Signature {
    type Bytes = [u8; 64];

    fn as_key_bytes(&self) -> &Self::Bytes {
        &self.to_bytes()
    }

    fn from_key_bytes(bytes: [u8; 64]) -> Result<Signature, Self::Bytes> {
        Ok(Signature::from_bytes(&bytes))
    }
}

impl KeyBytes for Commitment {
    type Bytes = [u8; 32];

    fn as_key_bytes(&self) -> &Self::Bytes {
        self
    }

    fn from_key_bytes(bytes: [u8; 32]) -> Result<Self, Self::Bytes> {
        Ok(bytes)
    }
}
