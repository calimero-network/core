use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use bs58::decode::Result as Bs58Result;

use calimero_context_config::repr;
use calimero_context_config::repr::{LengthMismatch, ReprBytes, ReprTransmute};
use calimero_context_config::types::IntoResult;
use candid::{CandidType, Principal};
use ed25519_dalek::{Signature, VerifyingKey, Verifier};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

/// Base identity type
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Copy, Hash)]
pub struct Identity([u8; 32]);

impl Identity {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    
    pub fn as_bytes(&self) -> [u8; 32] {
        self.0
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0[..]
    }
}

impl Default for Identity {
    fn default() -> Self {
        Self([0; 32])
    }
}

impl ReprBytes for Identity {
  type EncodeBytes<'a> = [u8; 32];
  type DecodeBytes = [u8; 32];
  type Error = LengthMismatch;

  fn as_bytes(&self) -> Self::EncodeBytes<'_> {
      self.0
  }

  fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
  where
      F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
  {
      Self::DecodeBytes::from_bytes(f).map(Self)
  }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq, Copy)]
pub struct ICSignerId(Identity);

impl ICSignerId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Identity(bytes))
    }
    
    pub fn as_bytes(&self) -> [u8; 32] {
        self.0.as_bytes()
    }
}

impl Default for ICSignerId {
    fn default() -> Self {
        Self(Identity::default())
    }
}


impl ReprBytes for ICSignerId {
  type EncodeBytes<'a> = [u8; 32];
  type DecodeBytes = [u8; 32];
  type Error = LengthMismatch;

  fn as_bytes(&self) -> Self::EncodeBytes<'_> {
      self.0.as_bytes()
  }

  fn from_bytes<F>(f: F) -> repr::Result<Self, Self::Error>
  where
      F: FnOnce(&mut Self::DecodeBytes) -> Bs58Result<usize>,
  {
      Identity::from_bytes(f).map(Self)
  }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
pub struct ICContextId(Identity);

impl ICContextId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Identity(bytes))
    }

    pub fn as_bytes(&self) -> [u8; 32] {
        self.0.as_bytes()
    }
}

impl Default for ICContextId {
    fn default() -> Self {
        Self(Identity::default())
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
pub struct ICProposalId(pub [u8; 32]);

impl ICProposalId {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ICProposalAction {
    ExternalFunctionCall {
        receiver_id: Principal,
        method_name: String,
        args: String,
        deposit: u128,
    },
    Transfer {
        receiver_id: Principal,
        amount: u128,
    },
    SetNumApprovals {
        num_approvals: u32,
    },
    SetActiveProposalsLimit {
        active_proposals_limit: u32,
    },
    SetContextValue {
        key: Vec<u8>,
        value: Vec<u8>,
    },
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct ICProposal {
    pub id: ICProposalId,
    pub author_id: ICSignerId,
    pub actions: Vec<ICProposalAction>,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct ICProposalWithApprovals {
    pub proposal_id: ICProposalId,
    pub num_approvals: usize,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct ICProposalApprovalWithSigner {
    pub proposal_id: ICProposalId,
    pub signer_id: ICSignerId,
    pub added_timestamp: u64,
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub enum ICRequestKind {
    Propose {
        proposal: ICProposal,
    },
    Approve {
        approval: ICProposalApprovalWithSigner,
    },
}

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

#[derive(CandidType, Serialize, Deserialize, Default)]
pub struct ICProxyContract {
    pub context_id: ICContextId,
    pub context_config_id: String,
    pub num_approvals: u32,
    pub proposals: HashMap<ICProposalId, ICProposal>,
    pub approvals: HashMap<ICProposalId, HashSet<ICSignerId>>,
    pub num_proposals_pk: HashMap<ICSignerId, u32>,
    pub active_proposals_limit: u32,
    pub context_storage: HashMap<Vec<u8>, Vec<u8>>,
    pub ledger_id: LedgerId,
}

impl ICProxyContract {
    pub fn new(context_id: ICContextId, ledger_id: Principal) -> Self {
        Self {
            context_id,
            context_config_id: ic_cdk::api::id().to_string(),
            num_approvals: 3,
            proposals: HashMap::new(),
            approvals: HashMap::new(),
            num_proposals_pk: HashMap::new(),
            active_proposals_limit: 10,
            context_storage: HashMap::new(),
            ledger_id: ledger_id.into(),
        }
    }
}

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct LedgerId(Principal);

impl Default for LedgerId {
    fn default() -> Self {
        Self(Principal::anonymous())
    }
}

impl From<Principal> for LedgerId {
    fn from(p: Principal) -> Self {
        Self(p)
    }
}

impl From<LedgerId> for Principal {
    fn from(id: LedgerId) -> Self {
        id.0
    }
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub struct TransferArgs {
    pub to: Principal,
    pub amount: u128,
}
