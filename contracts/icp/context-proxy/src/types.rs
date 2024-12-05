use std::marker::PhantomData;

use bs58::decode::Result as Bs58Result;
use calimero_context_config::repr;
use calimero_context_config::repr::{LengthMismatch, ReprBytes, ReprTransmute};
use calimero_context_config::types::IntoResult;
use candid::{CandidType, Principal};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct ICRequest {
    pub kind: ICRequestKind,
    pub signer_id: ICSignerId,
    pub timestamp_ms: u64,
}

#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct ICPSigned<T: CandidType + Serialize> {
    payload: Vec<u8>,
    signature: Vec<u8>,
    _phantom: Phantom<T>,
}

impl<T: CandidType + Serialize + DeserializeOwned> ICPSigned<T> {
    pub fn new<R, F>(payload: T, sign: F) -> Result<Self, ICPSignedError<R::Error>>
    where
        R: IntoResult<Signature>,
        F: FnOnce(&[u8]) -> R,
    {
        let bytes = candid::encode_one(payload)
            .map_err(|e| ICPSignedError::SerializationError(e.to_string()))?;

        let signature = sign(&bytes)
            .into_result()
            .map_err(ICPSignedError::DerivationError)?;

        Ok(Self {
            payload: bytes,
            signature: signature.to_vec(),
            _phantom: Phantom(PhantomData),
        })
    }

    pub fn parse<R, F>(&self, f: F) -> Result<T, ICPSignedError<R::Error>>
    where
        R: IntoResult<ICSignerId>,
        F: FnOnce(&T) -> R,
    {
        let parsed: T = candid::decode_one(&self.payload)
            .map_err(|e| ICPSignedError::DeserializationError(e.to_string()))?;

        let signer_id = f(&parsed)
            .into_result()
            .map_err(ICPSignedError::DerivationError)?;

        let key = signer_id
            .rt::<VerifyingKey>()
            .map_err(|_| ICPSignedError::InvalidPublicKey)?;

        let signature_bytes: [u8; 64] =
            self.signature.as_slice().try_into().map_err(|_| {
                ICPSignedError::SignatureError(ed25519_dalek::ed25519::Error::new())
            })?;
        let signature = ed25519_dalek::Signature::from_bytes(&signature_bytes);

        key.verify(&self.payload, &signature)
            .map_err(|_| ICPSignedError::InvalidSignature)?;

        Ok(parsed)
    }
}

#[derive(Debug, ThisError)]
pub enum ICPSignedError<E> {
    #[error("invalid signature")]
    InvalidSignature,
    #[error("derivation error: {0}")]
    DerivationError(E),
    #[error("invalid public key")]
    InvalidPublicKey,
    #[error("signature error: {0}")]
    SignatureError(#[from] ed25519_dalek::ed25519::Error),
    #[error("serialization error: {0}")]
    SerializationError(String),
    #[error("deserialization error: {0}")]
    DeserializationError(String),
}

#[derive(Deserialize, Debug, Clone)]
struct Phantom<T>(#[serde(skip)] std::marker::PhantomData<T>);

impl<T> CandidType for Phantom<T> {
    fn _ty() -> candid::types::Type {
        candid::types::TypeInner::Null.into()
    }

    fn idl_serialize<S>(&self, serializer: S) -> Result<(), S::Error>
    where
        S: candid::types::Serializer,
    {
        serializer.serialize_null(())
    }
}
