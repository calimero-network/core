use calimero_sdk::serde::{Deserialize, Serialize};
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};

use crate::repr::{Repr, ReprBytes};
use crate::Commitment;

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "calimero_sdk::serde")]
pub struct KeyComponents {
    pub pk: Repr<VerifyingKey>,
    pub sk: Repr<SigningKey>,
}

impl ReprBytes for VerifyingKey {
    type Bytes = [u8; 32];

    fn to_bytes(&self) -> Self::Bytes {
        self.to_bytes()
    }

    fn from_bytes(bytes: Self::Bytes) -> Result<VerifyingKey, Self::Bytes> {
        VerifyingKey::from_bytes(&bytes).map_err(|_| bytes)
    }
}

impl ReprBytes for SigningKey {
    type Bytes = [u8; 32];

    fn to_bytes(&self) -> Self::Bytes {
        self.to_bytes()
    }

    fn from_bytes(bytes: Self::Bytes) -> Result<SigningKey, Self::Bytes> {
        Ok(SigningKey::from_bytes(&bytes))
    }
}

impl ReprBytes for Signature {
    type Bytes = [u8; 64];

    fn to_bytes(&self) -> Self::Bytes {
        self.to_bytes()
    }

    fn from_bytes(bytes: Self::Bytes) -> Result<Signature, Self::Bytes> {
        Ok(Signature::from_bytes(&bytes))
    }
}

impl ReprBytes for Commitment {
    type Bytes = [u8; 32];

    fn to_bytes(&self) -> Self::Bytes {
        *self
    }

    fn from_bytes(bytes: Self::Bytes) -> Result<Self, Self::Bytes> {
        Ok(bytes)
    }
}
