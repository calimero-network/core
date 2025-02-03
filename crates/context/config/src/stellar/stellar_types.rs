extern crate alloc;
use alloc::borrow::Cow;
use alloc::vec::Vec as StdVec;

use bs58;
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{contracterror, contracttype, Bytes, BytesN, Env, String, Vec};

use super::StellarProxyMutateRequest;
use crate::repr::{Repr, ReprBytes, ReprError, ReprTransmute};
use crate::types::{Application, ApplicationMetadata, ApplicationSource, Capability};

// Core types for Application
#[contracttype]
#[derive(Clone, Debug)]
pub struct StellarApplication {
    pub id: BytesN<32>,
    pub blob: BytesN<32>,
    pub size: u64,
    pub source: String,
    pub metadata: Bytes,
}

// Request structures
#[contracttype]
#[derive(Clone, Debug)]
pub struct StellarContextRequest {
    pub context_id: BytesN<32>,
    pub kind: StellarContextRequestKind,
}

#[contracttype]
#[derive(Clone, Debug, Copy)]
pub enum StellarCapability {
    ManageApplication,
    ManageMembers,
    Proxy,
}

// Request types without named fields in enum variants
#[contracttype]
#[derive(Clone, Debug)]
pub enum StellarContextRequestKind {
    Add(BytesN<32>, StellarApplication),
    UpdateApplication(StellarApplication),
    AddMembers(Vec<BytesN<32>>),
    RemoveMembers(Vec<BytesN<32>>),
    Grant(Vec<(BytesN<32>, StellarCapability)>),
    Revoke(Vec<(BytesN<32>, StellarCapability)>),
    UpdateProxyContract,
}

#[contracttype]
#[derive(Clone, Debug)]
pub enum StellarRequestKind {
    Context(StellarContextRequest),
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct StellarRequest {
    pub kind: StellarRequestKind,
    pub signer_id: BytesN<32>,
    pub nonce: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub enum StellarSignedRequestPayload {
    Context(StellarRequest),
    Proxy(StellarProxyMutateRequest),
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct StellarSignedRequest {
    pub payload: StellarSignedRequestPayload,
    pub signature: BytesN<64>,
}

impl StellarSignedRequest {
    pub fn new<F>(
        env: &Env,
        payload: StellarSignedRequestPayload,
        sign: F,
    ) -> Result<Self, StellarError>
    where
        F: FnOnce(&[u8]) -> Result<ed25519_dalek::Signature, ed25519_dalek::SignatureError>,
    {
        let request_xdr = payload.clone().to_xdr(env);
        let std_vec: StdVec<u8> = request_xdr.into_iter().collect();

        let signature = sign(&std_vec).map_err(|_| StellarError::InvalidSignature)?;

        Ok(Self {
            payload,
            signature: BytesN::from_array(env, &signature.to_bytes()),
        })
    }

    pub fn verify(&self, env: &Env) -> Result<StellarSignedRequestPayload, StellarError> {
        let bytes = self.payload.clone().to_xdr(env);

        // Get signer_id based on payload type
        let signer_id = match &self.payload {
            StellarSignedRequestPayload::Context(req) => &req.signer_id,
            StellarSignedRequestPayload::Proxy(req) => match req {
                StellarProxyMutateRequest::Propose(proposal) => &proposal.author_id,
                StellarProxyMutateRequest::Approve(approval) => &approval.signer_id,
            },
        };

        env.crypto()
            .ed25519_verify(signer_id, &bytes, &self.signature);

        Ok(self.payload.clone())
    }
}

impl From<Application<'_>> for StellarApplication {
    fn from(value: Application<'_>) -> Self {
        let env = Env::default();
        StellarApplication {
            id: value
                .id
                .rt::<Repr<BytesN<32>>>()
                .expect("infallible conversion")
                .into_inner(),
            blob: value
                .blob
                .rt::<Repr<BytesN<32>>>()
                .expect("infallible conversion")
                .into_inner(),
            size: value.size,
            source: String::from_str(&env, &value.source.0.into_owned()),
            metadata: Bytes::from_slice(&env, &value.metadata.0.into_inner().into_owned()),
        }
    }
}

impl<'a> From<StellarApplication> for Application<'a> {
    fn from(value: StellarApplication) -> Self {
        // Convert Soroban String to std::String
        let mut bytes = vec![0u8; value.source.len() as usize];
        value.source.copy_into_slice(&mut bytes);
        let std_string = std::string::String::from_utf8(bytes).expect("valid utf8");

        Application::new(
            value.id.rt().expect("infallible conversion"),
            value.blob.rt().expect("infallible conversion"),
            value.size,
            ApplicationSource(Cow::Owned(std_string)),
            ApplicationMetadata(Repr::new(Cow::Owned(value.metadata.to_alloc_vec()))),
        )
    }
}

// We need to implement ReprBytes for BytesN<32>
impl ReprBytes for BytesN<32> {
    type EncodeBytes<'a>
        = [u8; 32]
    where
        Self: 'a;
    type DecodeBytes = [u8; 32];
    type Error = bs58::decode::Error;

    fn as_bytes(&self) -> Self::EncodeBytes<'_> {
        self.to_array()
    }

    fn from_bytes<F>(f: F) -> Result<Self, ReprError<Self::Error>>
    where
        F: FnOnce(&mut Self::DecodeBytes) -> Result<usize, bs58::decode::Error>,
    {
        let mut bytes = [0u8; 32];
        match f(&mut bytes) {
            Ok(_) => {
                let env = Env::default();
                Ok(BytesN::from_array(&env, &bytes))
            }
            Err(e) => Err(ReprError::InvalidBase58(e)),
        }
    }
}

// Similar implementations for other types that need conversion
impl From<Capability> for StellarCapability {
    fn from(value: Capability) -> Self {
        match value {
            Capability::ManageApplication => StellarCapability::ManageApplication,
            Capability::ManageMembers => StellarCapability::ManageMembers,
            Capability::Proxy => StellarCapability::Proxy,
        }
    }
}

impl From<StellarCapability> for Capability {
    fn from(value: StellarCapability) -> Self {
        match value {
            StellarCapability::ManageApplication => Capability::ManageApplication,
            StellarCapability::ManageMembers => Capability::ManageMembers,
            StellarCapability::Proxy => Capability::Proxy,
        }
    }
}

// Contract error enum (keep this for contract errors)
#[contracterror]
#[derive(Copy, Clone, Debug)]
pub enum StellarError {
    InvalidSignature = 1,
    Unauthorized = 2,
    ContextExists = 3,
    ContextNotFound = 4,
    InvalidNonce = 5,
    ProxyCodeNotSet = 6,
    NotAMember = 7,
    InvalidState = 8,
    ProxyUpgradeFailed = 9,
}
