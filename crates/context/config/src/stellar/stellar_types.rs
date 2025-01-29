extern crate alloc;
use alloc::borrow::Cow;
use alloc::vec::Vec as StdVec;

use bs58;
use ed25519_dalek::Signer;
use soroban_env_common::Env as CommonEnv;
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{
    contracterror, contracttype, Bytes, BytesN, Env, IntoVal, String, TryIntoVal, Val, Vec,
};

use super::stellar_repr::StellarRepr;
use crate::repr::{Repr, ReprBytes, ReprError, ReprTransmute};
use crate::types::{Application, ApplicationMetadata, ApplicationSource, Capability};
use crate::{ContextRequest, ContextRequestKind, RequestKind};

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

impl<'a> From<ContextRequest<'a>> for StellarContextRequest {
    fn from(value: ContextRequest<'a>) -> Self {
        let repr_context_id: [u8; 32] = value.context_id.rt().expect("infallible conversion");
        let context_id = BytesN::from_array(&Env::default(), &repr_context_id);
        let kind = value.kind.into();
        Self { context_id, kind }
    }
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

impl From<ContextRequestKind<'_>> for StellarContextRequestKind {
    fn from(value: ContextRequestKind<'_>) -> Self {
        match value {
            ContextRequestKind::Add {
                author_id,
                application,
            } => {
                let repr_author_id: [u8; 32] = author_id.rt().expect("infallible conversion");
                let author_id = BytesN::from_array(&Env::default(), &repr_author_id);
                let stellar_app: StellarApplication = application.into();

                StellarContextRequestKind::Add(author_id, stellar_app)
            }
            ContextRequestKind::UpdateApplication { application } => {
                StellarContextRequestKind::UpdateApplication(application.into())
            }
            ContextRequestKind::AddMembers { members } => {
                let mut vec = Vec::new(&Env::default());
                for member in members.into_owned() {
                    vec.push_back(BytesN::from_array(&Env::default(), &member.as_bytes()));
                }
                StellarContextRequestKind::AddMembers(vec)
            }
            ContextRequestKind::RemoveMembers { members } => {
                let mut vec = Vec::new(&Env::default());
                for member in members.into_owned() {
                    vec.push_back(BytesN::from_array(&Env::default(), &member.as_bytes()));
                }
                StellarContextRequestKind::RemoveMembers(vec)
            }
            ContextRequestKind::Grant { capabilities } => {
                let mut vec = Vec::new(&Env::default());
                for (id, cap) in capabilities.into_owned() {
                    vec.push_back((
                        BytesN::from_array(
                            &Env::default(),
                            &id.rt::<BytesN<32>>()
                                .expect("infallible conversion")
                                .as_bytes(),
                        ),
                        cap.into(),
                    ));
                }
                StellarContextRequestKind::Grant(vec)
            }
            ContextRequestKind::Revoke { capabilities } => {
                let mut vec = Vec::new(&Env::default());
                for (id, cap) in capabilities.into_owned() {
                    vec.push_back((
                        BytesN::from_array(
                            &Env::default(),
                            &id.rt::<BytesN<32>>()
                                .expect("infallible conversion")
                                .as_bytes(),
                        ),
                        cap.into(),
                    ));
                }
                StellarContextRequestKind::Revoke(vec)
            }
            ContextRequestKind::UpdateProxyContract => {
                StellarContextRequestKind::UpdateProxyContract
            }
        }
    }
}

#[contracttype]
#[derive(Clone, Debug)]
pub enum StellarRequestKind {
    Context(StellarContextRequest),
}

impl<'a> From<RequestKind<'a>> for StellarRequestKind {
    fn from(value: RequestKind<'a>) -> Self {
        match value {
            RequestKind::Context(context) => {
                let stellar_context = context.into();
                StellarRequestKind::Context(stellar_context)
            }
        }
    }
}

// TODO implement new method
#[contracttype]
#[derive(Clone, Debug)]
pub struct StellarRequest {
    pub signer_id: BytesN<32>,
    pub kind: StellarRequestKind,
    pub nonce: u64,
}

impl StellarRequest {
    pub fn new(signer_id: BytesN<32>, kind: StellarRequestKind, nonce: u64) -> Self {
        Self {
            signer_id,
            kind,
            nonce,
        }
    }
}

// Signed request wrapper
#[contracttype]
#[derive(Clone, Debug)]
pub struct StellarSignedRequest {
    pub payload: StellarRequest,
    pub signature: BytesN<64>,
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

impl StellarSignedRequest {
    pub fn new<F>(env: &Env, payload: StellarRequest, sign: F) -> Result<Self, StellarError>
    where
        F: FnOnce(&[u8]) -> Result<ed25519_dalek::Signature, ed25519_dalek::SignatureError>,
    {
        println!("=== Starting XDR Serialization ===");

        // Serialize kind first
        println!("Serializing kind...");
        let kind_xdr = match &payload.kind {
            StellarRequestKind::Context(context) => {
                println!("Context kind: {:?}", context);
                let context_xdr = context.clone().to_xdr(env);
                println!("Context XDR: {:?}", context_xdr);
                context_xdr
            }
        };

        // Serialize signer_id
        println!("Serializing signer_id...");
        let signer_xdr = payload.signer_id.clone().to_xdr(env);
        println!("Signer XDR: {:?}", signer_xdr);

        // Serialize nonce
        println!("Serializing nonce...");
        let nonce_xdr = payload.nonce.to_xdr(env);
        println!("Nonce XDR: {:?}", nonce_xdr);

        // Combine all parts
        let mut combined_vec = StdVec::new();
        combined_vec.extend(kind_xdr.into_iter());
        combined_vec.extend(signer_xdr.into_iter());
        combined_vec.extend(nonce_xdr.into_iter());

        println!("Combined XDR bytes: {:?}", combined_vec);

        let signature = sign(&combined_vec).map_err(|_| StellarError::InvalidSignature)?;

        Ok(Self {
            payload,
            signature: BytesN::from_array(env, &signature.to_bytes()),
        })
    }

    pub fn verify(&self, env: &Env) -> Result<StellarRequest, StellarError> {
        let bytes = self.payload.clone().to_xdr(env);

        env.crypto()
            .ed25519_verify(&self.payload.signer_id, &bytes, &self.signature);

        Ok(self.payload.clone())
    }
}

impl<'a> From<Application<'a>> for StellarApplication {
    fn from(value: Application<'a>) -> Self {
        let env = Env::default();
        let repr_id: [u8; 32] = value.id.rt().expect("infallible conversion");
        let id = BytesN::from_array(&env, &repr_id);
        let repr_blob: [u8; 32] = value.blob.rt().expect("infallible conversion");
        let blob = BytesN::from_array(&env, &repr_blob);
        let app = StellarApplication {
            id,
            blob,
            size: value.size,
            source: String::from_str(&env, &value.source.0.into_owned()),
            metadata: Bytes::new(&env),
        };

        app
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
            ApplicationMetadata(Repr::new(Cow::Owned(value.metadata.into_iter().collect()))),
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
